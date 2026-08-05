//! Durable replay plus bounded live delivery for interaction events.

use std::collections::{HashMap, VecDeque};
use std::sync::Arc;

use async_trait::async_trait;
use polkagent_core::ids::ConversationId;
use tokio::sync::{broadcast, Mutex};

use crate::error::{InteractionError, InteractionErrorCode};
use crate::event::InteractionEventEnvelope;
use crate::ids::InteractionTurnId;
use crate::model::SubscriptionRequest;
use crate::persistence::{InteractionStore, NewAssistantMessage, NewInteractionEvent};
use crate::service::{BoxInteractionEventStream, InteractionEventStream, StreamError};

const REPLAY_PAGE_SIZE: u16 = 1_000;

type LiveChannels = HashMap<ConversationId, Vec<broadcast::Sender<InteractionEventEnvelope>>>;

/// Coordinates durable append, replay, and bounded live fan-out.
///
/// Every event is committed through [`InteractionStore`] before it becomes
/// visible on a live receiver. Subscriptions attach their bounded receiver
/// before lazily paging replay so concurrent publications cannot be missed.
#[derive(Clone)]
pub struct InteractionEventHub {
    store: Arc<dyn InteractionStore>,
    channels: Arc<Mutex<LiveChannels>>,
}

impl std::fmt::Debug for InteractionEventHub {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("InteractionEventHub")
            .field("store", &"<interaction store>")
            .finish_non_exhaustive()
    }
}

impl InteractionEventHub {
    /// Create a hub over one authoritative durable store.
    #[must_use]
    pub fn new(store: Arc<dyn InteractionStore>) -> Self {
        Self {
            store,
            channels: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Persist an event, then offer it to every current bounded subscriber.
    ///
    /// Delivery failure never rolls back the durable append. A lagging caller
    /// receives an explicit [`StreamError::Lagged`] and recovers from the
    /// durable checkpoint on its next subscription.
    pub async fn publish(
        &self,
        event: NewInteractionEvent,
    ) -> Result<InteractionEventEnvelope, InteractionError> {
        let envelope = self.store.append_event(event).await?;
        self.fan_out(&envelope).await;
        Ok(envelope)
    }

    /// Atomically finish a turn in the durable store, then offer the terminal
    /// envelope to every live subscriber.
    pub async fn publish_terminal(
        &self,
        event: NewInteractionEvent,
        assistant_message: NewAssistantMessage,
    ) -> Result<InteractionEventEnvelope, InteractionError> {
        if !event.event.is_terminal() {
            return Err(InteractionError::invalid_request(
                "publish_terminal requires a terminal interaction event",
            ));
        }
        let envelope = self.store.finish_turn(event, assistant_message).await?;
        self.fan_out(&envelope).await;
        Ok(envelope)
    }

    async fn fan_out(&self, envelope: &InteractionEventEnvelope) {
        let mut channels = self.channels.lock().await;
        if let Some(senders) = channels.get_mut(&envelope.conversation_id) {
            senders.retain(|sender| {
                if sender.receiver_count() == 0 {
                    return false;
                }
                let _ = sender.send(envelope.clone());
                true
            });
            if senders.is_empty() {
                channels.remove(&envelope.conversation_id);
            }
        }
    }

    /// Attach a bounded receiver, then lazily replay and follow live work.
    ///
    /// Subscription itself performs no durable replay I/O. The returned stream
    /// keeps at most one replay page in memory and requests the next page only
    /// when a caller asks for another event.
    pub async fn subscribe(
        &self,
        request: SubscriptionRequest,
    ) -> Result<BoxInteractionEventStream, InteractionError> {
        request.validate()?;
        let (sender, receiver) = broadcast::channel(request.capacity);
        self.channels
            .lock()
            .await
            .entry(request.conversation_id)
            .or_default()
            .push(sender);

        let after_sequence = request.after_sequence.unwrap_or(0);
        Ok(Box::new(DurableInteractionEventStream {
            store: Arc::clone(&self.store),
            conversation_id: request.conversation_id,
            turn_id: request.turn_id,
            receiver,
            replay: VecDeque::new(),
            replay_complete: false,
            cursor: after_sequence,
            checkpoint: request.after_sequence,
        }))
    }
}

struct DurableInteractionEventStream {
    store: Arc<dyn InteractionStore>,
    conversation_id: ConversationId,
    turn_id: Option<InteractionTurnId>,
    receiver: broadcast::Receiver<InteractionEventEnvelope>,
    replay: VecDeque<InteractionEventEnvelope>,
    replay_complete: bool,
    cursor: u64,
    checkpoint: Option<u64>,
}

impl DurableInteractionEventStream {
    async fn load_next_replay_page(&mut self) -> Result<(), StreamError> {
        let page = self
            .store
            .load_events(
                self.conversation_id,
                self.cursor,
                u32::from(REPLAY_PAGE_SIZE),
            )
            .await
            .map_err(StreamError::Backend)?;
        if page.is_empty() {
            self.replay_complete = true;
            return Ok(());
        }
        if page.len() > usize::from(REPLAY_PAGE_SIZE) {
            return Err(StreamError::Backend(InteractionError::new(
                InteractionErrorCode::Internal,
                "durable interaction replay exceeded its requested page size",
            )));
        }
        let mut page_cursor = self.cursor;
        for event in &page {
            if event.conversation_id != self.conversation_id || event.sequence <= page_cursor {
                return Err(StreamError::Backend(InteractionError::new(
                    InteractionErrorCode::Internal,
                    "durable interaction replay did not advance its sequence",
                )));
            }
            page_cursor = event.sequence;
        }
        self.replay_complete = page.len() < usize::from(REPLAY_PAGE_SIZE);
        self.replay = VecDeque::from(page);
        Ok(())
    }

    fn deliver_if_visible(
        &mut self,
        event: InteractionEventEnvelope,
    ) -> Option<InteractionEventEnvelope> {
        if event.sequence <= self.cursor {
            return None;
        }
        self.cursor = event.sequence;
        if self.turn_id.is_some_and(|turn_id| turn_id != event.turn_id) {
            return None;
        }
        self.checkpoint = Some(event.sequence);
        Some(event)
    }
}

#[async_trait]
impl InteractionEventStream for DurableInteractionEventStream {
    async fn recv(&mut self) -> Result<InteractionEventEnvelope, StreamError> {
        loop {
            if let Some(event) = self.replay.pop_front() {
                if let Some(event) = self.deliver_if_visible(event) {
                    return Ok(event);
                }
                continue;
            }
            if !self.replay_complete {
                self.load_next_replay_page().await?;
                continue;
            }

            match self.receiver.recv().await {
                Ok(event) => {
                    if let Some(event) = self.deliver_if_visible(event) {
                        return Ok(event);
                    }
                }
                Err(broadcast::error::RecvError::Lagged(_)) => {
                    let latest = self
                        .store
                        .latest_event_sequence(self.conversation_id)
                        .await
                        .map_err(StreamError::Backend)?;
                    return Err(StreamError::Lagged {
                        last_seen_sequence: self.checkpoint,
                        resume_after_sequence: latest.max(self.cursor),
                    });
                }
                Err(broadcast::error::RecvError::Closed) => return Err(StreamError::Closed),
            }
        }
    }

    fn checkpoint(&self) -> Option<u64> {
        self.checkpoint
    }
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    reason = "event-hub tests intentionally fail at the exact fake-store or stream boundary"
)]
mod tests {
    use std::sync::atomic::{AtomicBool, Ordering};

    use chrono::Utc;
    use polkagent_core::ids::RunId;

    use super::*;
    use crate::event::InteractionEvent;
    use crate::ids::InteractionEventId;
    use crate::model::{
        InteractionConfig, InteractionState, InteractionSummary, ListInteractionsRequest,
    };
    use crate::persistence::{NewInteraction, NewInteractionTurn, StoredInteractionTurn};

    #[derive(Default)]
    struct FakeStore {
        events: Mutex<Vec<InteractionEventEnvelope>>,
        replay_requests: Mutex<Vec<(u64, u32)>>,
        fail_replay: AtomicBool,
    }

    impl FakeStore {
        async fn seed_events(
            &self,
            conversation_id: ConversationId,
            count: usize,
            turn_for_sequence: impl Fn(u64) -> InteractionTurnId,
        ) {
            let mut events = self.events.lock().await;
            for offset in 0..count {
                let sequence = u64::try_from(offset).expect("test sequence fits u64") + 1;
                events.push(InteractionEventEnvelope {
                    event_id: InteractionEventId::new(),
                    conversation_id,
                    turn_id: turn_for_sequence(sequence),
                    sequence,
                    timestamp: Utc::now(),
                    event: InteractionEvent::AgentMessageDelta {
                        run_id: RunId::new(),
                        text: format!("seed-{sequence}"),
                    },
                });
            }
        }

        async fn replay_requests(&self) -> Vec<(u64, u32)> {
            self.replay_requests.lock().await.clone()
        }
    }

    #[async_trait]
    impl InteractionStore for FakeStore {
        async fn create_interaction(
            &self,
            _interaction: NewInteraction,
        ) -> Result<InteractionSummary, InteractionError> {
            Err(unsupported())
        }

        async fn list_interactions(
            &self,
            _request: ListInteractionsRequest,
        ) -> Result<Vec<InteractionSummary>, InteractionError> {
            Err(unsupported())
        }

        async fn load_interaction(
            &self,
            _conversation_id: ConversationId,
        ) -> Result<InteractionSummary, InteractionError> {
            Err(unsupported())
        }

        async fn update_interaction_config(
            &self,
            _conversation_id: ConversationId,
            _config: InteractionConfig,
        ) -> Result<InteractionSummary, InteractionError> {
            Err(unsupported())
        }

        async fn set_interaction_state(
            &self,
            _conversation_id: ConversationId,
            _state: InteractionState,
        ) -> Result<InteractionSummary, InteractionError> {
            Err(unsupported())
        }

        async fn create_turn(
            &self,
            _turn: NewInteractionTurn,
        ) -> Result<StoredInteractionTurn, InteractionError> {
            Err(unsupported())
        }

        async fn load_turn(
            &self,
            _turn_id: InteractionTurnId,
        ) -> Result<StoredInteractionTurn, InteractionError> {
            Err(unsupported())
        }

        async fn list_turns(
            &self,
            _conversation_id: ConversationId,
        ) -> Result<Vec<StoredInteractionTurn>, InteractionError> {
            Err(unsupported())
        }

        async fn append_event(
            &self,
            event: NewInteractionEvent,
        ) -> Result<InteractionEventEnvelope, InteractionError> {
            let mut events = self.events.lock().await;
            if let Some(existing) = events
                .iter()
                .find(|stored| stored.event_id == event.event_id)
            {
                return Ok(existing.clone());
            }
            let sequence = u64::try_from(events.len())
                .unwrap_or(u64::MAX)
                .saturating_add(1);
            let envelope = InteractionEventEnvelope {
                event_id: event.event_id,
                conversation_id: event.conversation_id,
                turn_id: event.turn_id,
                sequence,
                timestamp: event.timestamp,
                event: event.event,
            };
            events.push(envelope.clone());
            Ok(envelope)
        }

        async fn load_events(
            &self,
            conversation_id: ConversationId,
            after_sequence: u64,
            limit: u32,
        ) -> Result<Vec<InteractionEventEnvelope>, InteractionError> {
            self.replay_requests
                .lock()
                .await
                .push((after_sequence, limit));
            if self.fail_replay.load(Ordering::SeqCst) {
                return Err(InteractionError::new(
                    InteractionErrorCode::Internal,
                    "deterministic replay storage failure",
                ));
            }
            let limit = usize::try_from(limit).unwrap_or(usize::MAX);
            Ok(self
                .events
                .lock()
                .await
                .iter()
                .filter(|event| {
                    event.conversation_id == conversation_id && event.sequence > after_sequence
                })
                .take(limit)
                .cloned()
                .collect())
        }

        async fn latest_event_sequence(
            &self,
            conversation_id: ConversationId,
        ) -> Result<u64, InteractionError> {
            Ok(self
                .events
                .lock()
                .await
                .iter()
                .rev()
                .find(|event| event.conversation_id == conversation_id)
                .map_or(0, |event| event.sequence))
        }
    }

    fn unsupported() -> InteractionError {
        InteractionError::new(
            InteractionErrorCode::Unsupported,
            "not used by event-hub tests",
        )
    }

    fn event(
        conversation_id: ConversationId,
        turn_id: InteractionTurnId,
        text: &str,
    ) -> NewInteractionEvent {
        NewInteractionEvent {
            event_id: InteractionEventId::new(),
            conversation_id,
            turn_id,
            timestamp: Utc::now(),
            event: InteractionEvent::AgentMessageDelta {
                run_id: RunId::new(),
                text: text.to_owned(),
            },
        }
    }

    #[tokio::test]
    async fn subscription_replays_then_follows_without_duplicates() {
        let store: Arc<dyn InteractionStore> = Arc::new(FakeStore::default());
        let hub = InteractionEventHub::new(Arc::clone(&store));
        let conversation_id = ConversationId::new();
        let turn_id = InteractionTurnId::new();
        let first = hub
            .publish(event(conversation_id, turn_id, "one"))
            .await
            .expect("publish replay event");
        let mut stream = hub
            .subscribe(SubscriptionRequest {
                conversation_id,
                turn_id: None,
                after_sequence: None,
                capacity: 8,
            })
            .await
            .expect("subscribe");
        assert_eq!(stream.recv().await.expect("receive replay"), first);

        let second = hub
            .publish(event(conversation_id, turn_id, "two"))
            .await
            .expect("publish live event");
        assert_eq!(stream.recv().await.expect("receive live"), second);
        assert_eq!(stream.checkpoint(), Some(2));
    }

    #[tokio::test]
    async fn lag_reports_latest_durable_recovery_checkpoint() {
        let store = Arc::new(FakeStore::default());
        let conversation_id = ConversationId::new();
        let turn_id = InteractionTurnId::new();
        store
            .append_event(event(conversation_id, turn_id, "already delivered"))
            .await
            .expect("append checkpoint event");
        let hub = InteractionEventHub::new(store.clone());
        let (sender, receiver) = broadcast::channel(1);
        let stream_store: Arc<dyn InteractionStore> = store.clone();
        let mut stream = DurableInteractionEventStream {
            store: stream_store,
            conversation_id,
            turn_id: None,
            receiver,
            replay: VecDeque::new(),
            replay_complete: true,
            cursor: 1,
            checkpoint: Some(1),
        };
        let second = store
            .append_event(event(conversation_id, turn_id, "two"))
            .await
            .expect("append two");
        sender.send(second.clone()).expect("send two");
        let third = store
            .append_event(event(conversation_id, turn_id, "three"))
            .await
            .expect("append three");
        sender.send(third.clone()).expect("send three");

        assert_eq!(
            stream.recv().await,
            Err(StreamError::Lagged {
                last_seen_sequence: Some(1),
                resume_after_sequence: 3,
            })
        );

        let mut resumed = hub
            .subscribe(SubscriptionRequest {
                conversation_id,
                turn_id: None,
                after_sequence: Some(1),
                capacity: 1,
            })
            .await
            .expect("resubscribe after delivered checkpoint");
        assert_eq!(resumed.recv().await.expect("replay second"), second);
        assert_eq!(resumed.recv().await.expect("replay third"), third);
    }

    #[tokio::test]
    async fn turn_filter_skips_other_turns_but_keeps_sequence_order() {
        let store: Arc<dyn InteractionStore> = Arc::new(FakeStore::default());
        let hub = InteractionEventHub::new(store);
        let conversation_id = ConversationId::new();
        let selected_turn = InteractionTurnId::new();
        hub.publish(event(conversation_id, InteractionTurnId::new(), "hidden"))
            .await
            .expect("publish hidden event");
        let visible = hub
            .publish(event(conversation_id, selected_turn, "visible"))
            .await
            .expect("publish visible event");

        let mut stream = hub
            .subscribe(SubscriptionRequest {
                conversation_id,
                turn_id: Some(selected_turn),
                after_sequence: None,
                capacity: 4,
            })
            .await
            .expect("subscribe");
        assert_eq!(
            stream.recv().await.expect("receive filtered replay"),
            visible
        );
        assert_eq!(stream.checkpoint(), Some(2));
    }

    #[tokio::test]
    async fn replay_pages_are_loaded_only_when_the_caller_reaches_them() {
        let store = Arc::new(FakeStore::default());
        let conversation_id = ConversationId::new();
        let turn_id = InteractionTurnId::new();
        let page_size = usize::from(REPLAY_PAGE_SIZE);
        store
            .seed_events(conversation_id, page_size * 2 + 5, |_| turn_id)
            .await;
        let hub = InteractionEventHub::new(store.clone());
        let mut stream = hub
            .subscribe(SubscriptionRequest {
                conversation_id,
                turn_id: None,
                after_sequence: None,
                capacity: 8,
            })
            .await
            .expect("subscribe without replay I/O");
        assert!(store.replay_requests().await.is_empty());

        assert_eq!(stream.recv().await.expect("first replay").sequence, 1);
        assert_eq!(
            store.replay_requests().await,
            vec![(0, u32::from(REPLAY_PAGE_SIZE))]
        );
        for expected in 2..=page_size {
            assert_eq!(
                stream.recv().await.expect("buffered first page").sequence,
                u64::try_from(expected).expect("expected sequence fits u64")
            );
        }
        assert_eq!(store.replay_requests().await.len(), 1);

        assert_eq!(
            stream
                .recv()
                .await
                .expect("first event of page two")
                .sequence,
            u64::try_from(page_size + 1).expect("page sequence fits u64")
        );
        assert_eq!(store.replay_requests().await.len(), 2);
        for _ in (page_size + 2)..=(page_size * 2) {
            stream.recv().await.expect("buffered second page");
        }
        assert_eq!(store.replay_requests().await.len(), 2);

        assert_eq!(
            stream
                .recv()
                .await
                .expect("first event of short final page")
                .sequence,
            u64::try_from(page_size * 2 + 1).expect("final sequence fits u64")
        );
        assert_eq!(
            store.replay_requests().await,
            vec![
                (0, u32::from(REPLAY_PAGE_SIZE)),
                (
                    u64::try_from(page_size).expect("page size fits u64"),
                    u32::from(REPLAY_PAGE_SIZE),
                ),
                (
                    u64::try_from(page_size * 2).expect("two pages fit u64"),
                    u32::from(REPLAY_PAGE_SIZE),
                ),
            ]
        );
    }

    #[tokio::test]
    async fn publication_during_replay_is_delivered_once_before_later_live_work() {
        let store = Arc::new(FakeStore::default());
        let conversation_id = ConversationId::new();
        let turn_id = InteractionTurnId::new();
        let initial_count = usize::from(REPLAY_PAGE_SIZE) * 2 + 5;
        store
            .seed_events(conversation_id, initial_count, |_| turn_id)
            .await;
        let hub = InteractionEventHub::new(store);
        let mut stream = hub
            .subscribe(SubscriptionRequest {
                conversation_id,
                turn_id: None,
                after_sequence: None,
                capacity: 8,
            })
            .await
            .expect("subscribe");
        let mut sequences = vec![stream.recv().await.expect("start replay").sequence];

        let during_replay = hub
            .publish(event(conversation_id, turn_id, "published during replay"))
            .await
            .expect("publish during replay");
        for _ in 1..=initial_count {
            sequences.push(stream.recv().await.expect("continue replay").sequence);
        }
        assert_eq!(sequences.last(), Some(&during_replay.sequence));

        let after_replay = hub
            .publish(event(conversation_id, turn_id, "published after replay"))
            .await
            .expect("publish after replay");
        sequences.push(
            stream
                .recv()
                .await
                .expect("skip replay/live duplicate")
                .sequence,
        );
        assert_eq!(sequences.last(), Some(&after_replay.sequence));
        assert_eq!(sequences, (1..=after_replay.sequence).collect::<Vec<_>>());
    }

    #[tokio::test]
    async fn filtered_replay_advances_page_cursor_across_hidden_events() {
        let store = Arc::new(FakeStore::default());
        let conversation_id = ConversationId::new();
        let selected_turn = InteractionTurnId::new();
        let hidden_turn = InteractionTurnId::new();
        let page_size = u64::from(REPLAY_PAGE_SIZE);
        store
            .seed_events(
                conversation_id,
                usize::from(REPLAY_PAGE_SIZE) * 2 + 2,
                |sequence| {
                    if matches!(sequence, 1)
                        || sequence == page_size + 1
                        || sequence == page_size * 2 + 2
                    {
                        selected_turn
                    } else {
                        hidden_turn
                    }
                },
            )
            .await;
        let hub = InteractionEventHub::new(store.clone());
        let mut stream = hub
            .subscribe(SubscriptionRequest {
                conversation_id,
                turn_id: Some(selected_turn),
                after_sequence: None,
                capacity: 4,
            })
            .await
            .expect("subscribe with filter");

        assert_eq!(stream.recv().await.expect("selected page one").sequence, 1);
        assert_eq!(
            stream.recv().await.expect("selected page two").sequence,
            page_size + 1
        );
        assert_eq!(
            stream.recv().await.expect("selected page three").sequence,
            page_size * 2 + 2
        );
        assert_eq!(
            store.replay_requests().await,
            vec![
                (0, u32::from(REPLAY_PAGE_SIZE)),
                (page_size, u32::from(REPLAY_PAGE_SIZE)),
                (page_size * 2, u32::from(REPLAY_PAGE_SIZE)),
            ]
        );

        hub.publish(event(conversation_id, hidden_turn, "hidden live"))
            .await
            .expect("publish hidden live event");
        let visible = hub
            .publish(event(conversation_id, selected_turn, "visible live"))
            .await
            .expect("publish visible live event");
        assert_eq!(stream.recv().await.expect("visible live event"), visible);
        assert_eq!(stream.checkpoint(), Some(visible.sequence));
    }

    #[tokio::test]
    async fn dropping_after_one_event_does_not_load_the_remaining_backlog() {
        let store = Arc::new(FakeStore::default());
        let conversation_id = ConversationId::new();
        let turn_id = InteractionTurnId::new();
        store
            .seed_events(
                conversation_id,
                usize::from(REPLAY_PAGE_SIZE) * 3 + 1,
                |_| turn_id,
            )
            .await;
        let hub = InteractionEventHub::new(store.clone());
        let mut stream = hub
            .subscribe(SubscriptionRequest {
                conversation_id,
                turn_id: None,
                after_sequence: None,
                capacity: 2,
            })
            .await
            .expect("subscribe");
        assert!(store.replay_requests().await.is_empty());
        assert_eq!(stream.recv().await.expect("one replay event").sequence, 1);
        drop(stream);
        tokio::task::yield_now().await;
        assert_eq!(
            store.replay_requests().await,
            vec![(0, u32::from(REPLAY_PAGE_SIZE))]
        );
    }

    #[tokio::test]
    async fn replay_backend_failure_surfaces_from_recv_and_can_retry_exactly() {
        let store = Arc::new(FakeStore::default());
        let conversation_id = ConversationId::new();
        let turn_id = InteractionTurnId::new();
        store.seed_events(conversation_id, 1, |_| turn_id).await;
        store.fail_replay.store(true, Ordering::SeqCst);
        let hub = InteractionEventHub::new(store.clone());
        let mut stream = hub
            .subscribe(SubscriptionRequest {
                conversation_id,
                turn_id: None,
                after_sequence: None,
                capacity: 2,
            })
            .await
            .expect("subscription defers replay I/O");
        assert!(store.replay_requests().await.is_empty());
        let Err(StreamError::Backend(error)) = stream.recv().await else {
            panic!("replay storage failure must be typed as a stream backend error");
        };
        assert_eq!(error.code, InteractionErrorCode::Internal);

        store.fail_replay.store(false, Ordering::SeqCst);
        assert_eq!(stream.recv().await.expect("retry replay").sequence, 1);
        assert_eq!(
            store.replay_requests().await,
            vec![
                (0, u32::from(REPLAY_PAGE_SIZE)),
                (0, u32::from(REPLAY_PAGE_SIZE)),
            ]
        );
    }
}
