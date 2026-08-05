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
use crate::persistence::{InteractionStore, NewInteractionEvent};
use crate::service::{BoxInteractionEventStream, InteractionEventStream, StreamError};

const REPLAY_PAGE_SIZE: u16 = 1_000;

type LiveChannels = HashMap<ConversationId, Vec<broadcast::Sender<InteractionEventEnvelope>>>;

/// Coordinates durable append, replay, and bounded live fan-out.
///
/// Every event is committed through [`InteractionStore`] before it becomes
/// visible on a live receiver. Subscriptions attach their bounded receiver
/// before loading replay so concurrent publications cannot be missed.
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
        Ok(envelope)
    }

    /// Attach a bounded receiver, replay durable events, then follow live work.
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
        let replay =
            load_replay(self.store.as_ref(), request.conversation_id, after_sequence).await?;
        Ok(Box::new(DurableInteractionEventStream {
            store: Arc::clone(&self.store),
            conversation_id: request.conversation_id,
            turn_id: request.turn_id,
            receiver,
            replay,
            cursor: after_sequence,
            checkpoint: request.after_sequence,
        }))
    }
}

async fn load_replay(
    store: &dyn InteractionStore,
    conversation_id: ConversationId,
    after_sequence: u64,
) -> Result<VecDeque<InteractionEventEnvelope>, InteractionError> {
    let mut cursor = after_sequence;
    let mut replay = VecDeque::new();
    loop {
        let page = store
            .load_events(conversation_id, cursor, u32::from(REPLAY_PAGE_SIZE))
            .await?;
        if page.is_empty() {
            break;
        }
        let page_is_full = page.len() == usize::from(REPLAY_PAGE_SIZE);
        for event in page {
            if event.sequence <= cursor {
                return Err(InteractionError::new(
                    InteractionErrorCode::Internal,
                    "durable interaction replay did not advance its sequence",
                ));
            }
            cursor = event.sequence;
            replay.push_back(event);
        }
        if !page_is_full {
            break;
        }
    }
    Ok(replay)
}

struct DurableInteractionEventStream {
    store: Arc<dyn InteractionStore>,
    conversation_id: ConversationId,
    turn_id: Option<InteractionTurnId>,
    receiver: broadcast::Receiver<InteractionEventEnvelope>,
    replay: VecDeque<InteractionEventEnvelope>,
    cursor: u64,
    checkpoint: Option<u64>,
}

impl DurableInteractionEventStream {
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
    use chrono::Utc;
    use polkagent_core::ids::RunId;

    use super::*;
    use crate::event::InteractionEvent;
    use crate::ids::InteractionEventId;
    use crate::persistence::{NewInteractionTurn, StoredInteractionTurn};

    #[derive(Default)]
    struct FakeStore {
        events: Mutex<Vec<InteractionEventEnvelope>>,
    }

    #[async_trait]
    impl InteractionStore for FakeStore {
        async fn create_turn(
            &self,
            _turn: NewInteractionTurn,
        ) -> Result<StoredInteractionTurn, InteractionError> {
            Err(InteractionError::new(
                InteractionErrorCode::Unsupported,
                "not used by event-hub tests",
            ))
        }

        async fn load_turn(
            &self,
            _turn_id: InteractionTurnId,
        ) -> Result<StoredInteractionTurn, InteractionError> {
            Err(InteractionError::new(
                InteractionErrorCode::Unsupported,
                "not used by event-hub tests",
            ))
        }

        async fn list_turns(
            &self,
            _conversation_id: ConversationId,
        ) -> Result<Vec<StoredInteractionTurn>, InteractionError> {
            Err(InteractionError::new(
                InteractionErrorCode::Unsupported,
                "not used by event-hub tests",
            ))
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
        let store: Arc<dyn InteractionStore> = Arc::new(FakeStore::default());
        let hub = InteractionEventHub::new(store);
        let conversation_id = ConversationId::new();
        let turn_id = InteractionTurnId::new();
        let mut stream = hub
            .subscribe(SubscriptionRequest {
                conversation_id,
                turn_id: None,
                after_sequence: None,
                capacity: 1,
            })
            .await
            .expect("subscribe");
        hub.publish(event(conversation_id, turn_id, "one"))
            .await
            .expect("publish one");
        hub.publish(event(conversation_id, turn_id, "two"))
            .await
            .expect("publish two");

        assert_eq!(
            stream.recv().await,
            Err(StreamError::Lagged {
                last_seen_sequence: None,
                resume_after_sequence: 2,
            })
        );
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
}
