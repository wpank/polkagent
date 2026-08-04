//! Dependency resolution for skill packages.
//!
//! Given a set of skill manifests, the resolver performs:
//!
//! 1. **Version compatibility checking** — each dependency's version
//!    requirement (semver range) is matched against the available skill
//!    versions.
//! 2. **Cycle detection** — a topological sort over the dependency graph
//!    ensures that no circular dependencies exist.
//!
//! The output is an ordered list of [`SkillId`] values in dependency-first
//! order (i.e., a skill appears after all of its dependencies).

use std::collections::{HashMap, HashSet};

use semver::VersionReq;
use tracing::debug;

use crate::error::SkillError;
use crate::manifest::{SkillId, SkillManifest};

// ---------------------------------------------------------------------------
// SkillDependency
// ---------------------------------------------------------------------------

/// A resolved dependency reference: a name and a semver version requirement.
#[derive(Debug, Clone)]
pub struct SkillDependency {
    /// The dependency skill name.
    pub name: String,
    /// The semver version requirement (e.g. `>=0.2.0`, `^1.0`).
    pub version_req: VersionReq,
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Resolve the dependency graph for a set of skill manifests.
///
/// Returns an ordered list of [`SkillId`] values where each skill appears
/// after all of its transitive dependencies (topological order).
///
/// # Errors
///
/// - [`SkillError::CyclicDependency`] if the graph contains a cycle.
/// - [`SkillError::SkillNotFound`] if a dependency references a skill not
///   present in the input set.
/// - [`SkillError::VersionConflict`] if a dependency's version requirement
///   does not match the available version.
/// - [`SkillError::InvalidVersionReq`] if a dependency version string is
///   not a valid semver requirement.
pub fn resolve(manifests: &[SkillManifest]) -> Result<Vec<SkillId>, SkillError> {
    // Build an index: name -> (manifest, parsed SkillId).
    let mut index: HashMap<&str, (&SkillManifest, SkillId)> = HashMap::new();
    for m in manifests {
        let id = m.id()?;
        index.insert(&m.skill.name, (m, id));
    }

    // Check that all dependencies exist and satisfy version requirements.
    for manifest in manifests {
        for (dep_name, dep_spec) in &manifest.dependencies {
            let version_req = VersionReq::parse(&dep_spec.version).map_err(|e| {
                SkillError::InvalidVersionReq {
                    requirement: dep_spec.version.clone(),
                    reason: e.to_string(),
                }
            })?;

            match index.get(dep_name.as_str()) {
                None => {
                    return Err(SkillError::SkillNotFound {
                        name: dep_name.clone(),
                    });
                }
                Some((_, dep_id)) => {
                    if !version_req.matches(&dep_id.version) {
                        return Err(SkillError::VersionConflict {
                            name: dep_name.clone(),
                            required: dep_spec.version.clone(),
                            found: dep_id.version.to_string(),
                        });
                    }
                }
            }
        }
    }

    // Topological sort with cycle detection (Kahn's algorithm).
    topological_sort(manifests, &index)
}

// ---------------------------------------------------------------------------
// Topological sort (Kahn's algorithm)
// ---------------------------------------------------------------------------

fn topological_sort(
    manifests: &[SkillManifest],
    index: &HashMap<&str, (&SkillManifest, SkillId)>,
) -> Result<Vec<SkillId>, SkillError> {
    let names: Vec<&str> = manifests.iter().map(|m| m.skill.name.as_str()).collect();

    // Build adjacency and in-degree.
    let mut in_degree: HashMap<&str, usize> = HashMap::new();
    let mut dependents: HashMap<&str, Vec<&str>> = HashMap::new();

    for name in &names {
        in_degree.entry(name).or_insert(0);
        dependents.entry(name).or_default();
    }

    for manifest in manifests {
        for dep_name in manifest.dependencies.keys() {
            // dep_name -> manifest.skill.name (manifest depends on dep_name)
            *in_degree.entry(manifest.skill.name.as_str()).or_insert(0) += 1;
            dependents
                .entry(dep_name.as_str())
                .or_default()
                .push(manifest.skill.name.as_str());
        }
    }

    // Start with nodes that have no incoming edges.
    let mut queue: Vec<&str> = names
        .iter()
        .filter(|name| in_degree.get(*name).copied().unwrap_or(0) == 0)
        .copied()
        .collect();

    // Sort in reverse so that `pop()` (which takes from the end) yields
    // the lexicographically smallest name first, giving deterministic output.
    queue.sort_by(|a, b| b.cmp(a));

    let mut result: Vec<SkillId> = Vec::new();

    while let Some(name) = queue.pop() {
        if let Some((_, id)) = index.get(name) {
            result.push(id.clone());
        }

        if let Some(deps) = dependents.get(name) {
            for &dependent in deps {
                if let Some(deg) = in_degree.get_mut(dependent) {
                    *deg = deg.saturating_sub(1);
                    if *deg == 0 {
                        queue.push(dependent);
                        // Re-sort in reverse to maintain the invariant.
                        queue.sort_by(|a, b| b.cmp(a));
                    }
                }
            }
        }
    }

    // If we didn't process all nodes, there is a cycle.
    if result.len() != names.len() {
        let remaining: Vec<&str> = names
            .iter()
            .filter(|name| in_degree.get(*name).copied().unwrap_or(0) > 0)
            .copied()
            .collect();

        let cycle = find_cycle_description(manifests, &remaining);
        return Err(SkillError::CyclicDependency { cycle });
    }

    debug!(order = ?result.iter().map(|id| id.to_string()).collect::<Vec<_>>(), "resolved skill order");

    Ok(result)
}

/// Build a human-readable cycle description from the remaining unresolved
/// nodes.
fn find_cycle_description(manifests: &[SkillManifest], remaining: &[&str]) -> String {
    let remaining_set: HashSet<&str> = remaining.iter().copied().collect();

    // Try to trace a cycle starting from the first remaining node.
    if let Some(&start) = remaining.first() {
        let mut path = vec![start];
        let mut current = start;

        for _ in 0..remaining.len() + 1 {
            // Find a dependency of `current` that is also in the remaining set.
            let manifest = manifests.iter().find(|m| m.skill.name == current);

            if let Some(m) = manifest {
                if let Some(next) = m
                    .dependencies
                    .keys()
                    .find(|dep| remaining_set.contains(dep.as_str()))
                {
                    if path.contains(&next.as_str()) {
                        path.push(next.as_str());
                        return path.join(" -> ");
                    }
                    path.push(next.as_str());
                    current = next.as_str();
                } else {
                    break;
                }
            } else {
                break;
            }
        }

        path.join(" -> ")
    } else {
        "unknown cycle".to_string()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// Helper: create a manifest with optional dependencies.
    fn make_manifest(name: &str, version: &str, deps: &[(&str, &str)]) -> SkillManifest {
        let mut dependencies = HashMap::new();
        for (dep_name, ver_req) in deps {
            dependencies.insert(
                (*dep_name).to_string(),
                crate::manifest::DependencySpec {
                    version: (*ver_req).to_string(),
                },
            );
        }

        SkillManifest {
            skill: crate::manifest::SkillSection {
                name: name.to_string(),
                version: version.to_string(),
                description: String::new(),
                authors: Vec::new(),
                license: String::new(),
            },
            capabilities: Default::default(),
            prompts: Default::default(),
            config: HashMap::new(),
            dependencies,
        }
    }

    #[test]
    fn resolve_no_dependencies() {
        let a = make_manifest("alpha", "1.0.0", &[]);
        let b = make_manifest("beta", "2.0.0", &[]);

        let order = resolve(&[a, b]).expect("should resolve");
        assert_eq!(order.len(), 2);
        // Lexicographic order when there are no edges.
        assert_eq!(order[0].name, "alpha");
        assert_eq!(order[1].name, "beta");
    }

    #[test]
    fn resolve_linear_chain() {
        // c depends on b, b depends on a.
        let a = make_manifest("a", "1.0.0", &[]);
        let b = make_manifest("b", "1.0.0", &[("a", "^1.0")]);
        let c = make_manifest("c", "1.0.0", &[("b", "^1.0")]);

        let order = resolve(&[c, a, b]).expect("should resolve");
        let names: Vec<&str> = order.iter().map(|id| id.name.as_str()).collect();
        assert_eq!(names, vec!["a", "b", "c"]);
    }

    #[test]
    fn resolve_diamond_dependency() {
        //   a
        //  / \
        // b   c
        //  \ /
        //   d
        let a = make_manifest("a", "1.0.0", &[]);
        let b = make_manifest("b", "1.0.0", &[("a", "^1.0")]);
        let c = make_manifest("c", "1.0.0", &[("a", "^1.0")]);
        let d = make_manifest("d", "1.0.0", &[("b", "^1.0"), ("c", "^1.0")]);

        let order = resolve(&[d, b, c, a]).expect("should resolve");
        let names: Vec<&str> = order.iter().map(|id| id.name.as_str()).collect();

        // a must come before b and c; b and c must come before d.
        let pos_a = names.iter().position(|&n| n == "a").expect("a present");
        let pos_b = names.iter().position(|&n| n == "b").expect("b present");
        let pos_c = names.iter().position(|&n| n == "c").expect("c present");
        let pos_d = names.iter().position(|&n| n == "d").expect("d present");

        assert!(pos_a < pos_b);
        assert!(pos_a < pos_c);
        assert!(pos_b < pos_d);
        assert!(pos_c < pos_d);
    }

    #[test]
    fn detect_simple_cycle() {
        // a -> b -> a
        let a = make_manifest("a", "1.0.0", &[("b", "^1.0")]);
        let b = make_manifest("b", "1.0.0", &[("a", "^1.0")]);

        let result = resolve(&[a, b]);
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("cyclic"));
    }

    #[test]
    fn detect_three_node_cycle() {
        // a -> b -> c -> a
        let a = make_manifest("a", "1.0.0", &[("c", "^1.0")]);
        let b = make_manifest("b", "1.0.0", &[("a", "^1.0")]);
        let c = make_manifest("c", "1.0.0", &[("b", "^1.0")]);

        let result = resolve(&[a, b, c]);
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("cyclic"));
    }

    #[test]
    fn missing_dependency_error() {
        let a = make_manifest("a", "1.0.0", &[("nonexistent", "^1.0")]);

        let result = resolve(&[a]);
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("not found"));
        assert!(msg.contains("nonexistent"));
    }

    #[test]
    fn version_conflict_error() {
        // b requires a >=2.0.0, but only 1.0.0 is available.
        let a = make_manifest("a", "1.0.0", &[]);
        let b = make_manifest("b", "1.0.0", &[("a", ">=2.0.0")]);

        let result = resolve(&[a, b]);
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("version conflict") || msg.contains("Version conflict"));
    }

    #[test]
    fn version_requirement_satisfied() {
        // b requires a ^1.0, and 1.2.3 is available.
        let a = make_manifest("a", "1.2.3", &[]);
        let b = make_manifest("b", "1.0.0", &[("a", "^1.0")]);

        let order = resolve(&[a, b]).expect("should resolve");
        assert_eq!(order.len(), 2);
        assert_eq!(order[0].name, "a");
        assert_eq!(order[1].name, "b");
    }

    #[test]
    fn resolve_single_skill_no_deps() {
        let a = make_manifest("solo", "0.1.0", &[]);
        let order = resolve(&[a]).expect("should resolve");
        assert_eq!(order.len(), 1);
        assert_eq!(order[0].name, "solo");
    }

    #[test]
    fn resolve_empty_input() {
        let order = resolve(&[]).expect("should resolve");
        assert!(order.is_empty());
    }

    #[test]
    fn self_cycle_detected() {
        // a depends on itself.
        let a = make_manifest("a", "1.0.0", &[("a", "^1.0")]);

        let result = resolve(&[a]);
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("cyclic"));
    }
}
