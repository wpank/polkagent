//! Dependency resolution for plugin packages.
//!
//! Given a set of plugin manifests, the resolver performs:
//!
//! 1. **Version compatibility checking** — each dependency's version
//!    requirement (semver range) is matched against the available plugin
//!    versions.
//! 2. **Cycle detection** — a topological sort over the dependency graph
//!    ensures that no circular dependencies exist.
//!
//! The output is an ordered list of [`PluginId`] values in dependency-first
//! order (i.e., a plugin appears after all of its dependencies).

use std::collections::{HashMap, HashSet};

use semver::VersionReq;
use tracing::debug;

use crate::error::PluginError;
use crate::manifest::{PluginId, PluginManifest};

// ---------------------------------------------------------------------------
// DependencyResolver
// ---------------------------------------------------------------------------

/// Resolves plugin dependency graphs by performing version compatibility
/// checks and topological sorting.
#[derive(Debug, Default)]
pub struct DependencyResolver;

impl DependencyResolver {
    /// Create a new `DependencyResolver`.
    pub fn new() -> Self {
        Self
    }

    /// Resolve the dependency graph for a set of plugin manifests.
    ///
    /// Returns an ordered list of [`PluginId`] values where each plugin
    /// appears after all of its transitive dependencies (topological order).
    ///
    /// # Errors
    ///
    /// - [`PluginError::CyclicDependency`] if the graph contains a cycle.
    /// - [`PluginError::NotFound`] if a dependency references a plugin not
    ///   present in the input set.
    /// - [`PluginError::DependencyConflict`] if a dependency's version
    ///   requirement does not match the available version.
    pub fn resolve(&self, manifests: &[PluginManifest]) -> Result<Vec<PluginId>, PluginError> {
        // Build an index: name -> (manifest, parsed PluginId).
        let mut index: HashMap<&str, (&PluginManifest, PluginId)> = HashMap::new();
        for m in manifests {
            let id = m.id()?;
            index.insert(&m.plugin.name, (m, id));
        }

        // Check that all dependencies exist and satisfy version requirements.
        for manifest in manifests {
            for (dep_name, ver_req_str) in &manifest.dependencies {
                let version_req =
                    VersionReq::parse(ver_req_str).map_err(|e| PluginError::InvalidVersionReq {
                        requirement: format!("{dep_name}: {ver_req_str}"),
                        reason: e.to_string(),
                    })?;

                match index.get(dep_name.as_str()) {
                    None => {
                        return Err(PluginError::NotFound {
                            name: dep_name.clone(),
                        });
                    }
                    Some((_, dep_id)) => {
                        if !version_req.matches(&dep_id.version) {
                            return Err(PluginError::DependencyConflict {
                                dependency: dep_name.clone(),
                                required: ver_req_str.clone(),
                                found: dep_id.version.to_string(),
                            });
                        }
                    }
                }
            }
        }

        // Topological sort with cycle detection (Kahn's algorithm).
        self.topological_sort(manifests, &index)
    }

    /// Check whether a specific version satisfies a version requirement
    /// string.
    pub fn is_compatible(version: &semver::Version, requirement: &str) -> Result<bool, PluginError> {
        let req = VersionReq::parse(requirement).map_err(|e| PluginError::InvalidVersionReq {
            requirement: requirement.to_string(),
            reason: e.to_string(),
        })?;
        Ok(req.matches(version))
    }

    // -----------------------------------------------------------------------
    // Topological sort (Kahn's algorithm)
    // -----------------------------------------------------------------------

    fn topological_sort(
        &self,
        manifests: &[PluginManifest],
        index: &HashMap<&str, (&PluginManifest, PluginId)>,
    ) -> Result<Vec<PluginId>, PluginError> {
        let names: Vec<&str> = manifests.iter().map(|m| m.plugin.name.as_str()).collect();

        // Build adjacency and in-degree.
        let mut in_degree: HashMap<&str, usize> = HashMap::new();
        let mut dependents: HashMap<&str, Vec<&str>> = HashMap::new();

        for name in &names {
            in_degree.entry(name).or_insert(0);
            dependents.entry(name).or_default();
        }

        for manifest in manifests {
            for dep_name in manifest.dependencies.keys() {
                *in_degree
                    .entry(manifest.plugin.name.as_str())
                    .or_insert(0) += 1;
                dependents
                    .entry(dep_name.as_str())
                    .or_default()
                    .push(manifest.plugin.name.as_str());
            }
        }

        // Start with nodes that have no incoming edges.
        let mut queue: Vec<&str> = names
            .iter()
            .filter(|name| in_degree.get(*name).copied().unwrap_or(0) == 0)
            .copied()
            .collect();

        // Sort in reverse so `pop()` yields lexicographically smallest first.
        queue.sort_by(|a, b| b.cmp(a));

        let mut result: Vec<PluginId> = Vec::new();

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

            let cycle = Self::find_cycle_description(manifests, &remaining);
            return Err(PluginError::CyclicDependency { cycle });
        }

        debug!(
            order = ?result.iter().map(|id| id.to_string()).collect::<Vec<_>>(),
            "resolved plugin order"
        );

        Ok(result)
    }

    /// Build a human-readable cycle description from the remaining unresolved
    /// nodes.
    fn find_cycle_description(manifests: &[PluginManifest], remaining: &[&str]) -> String {
        let remaining_set: HashSet<&str> = remaining.iter().copied().collect();

        if let Some(&start) = remaining.first() {
            let mut path = vec![start];
            let mut current = start;

            for _ in 0..remaining.len() + 1 {
                let manifest = manifests.iter().find(|m| m.plugin.name == current);

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
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    /// Helper: create a manifest with optional dependencies.
    fn make_manifest(name: &str, version: &str, deps: &[(&str, &str)]) -> PluginManifest {
        let mut dependencies = HashMap::new();
        for (dep_name, ver_req) in deps {
            dependencies.insert((*dep_name).to_string(), (*ver_req).to_string());
        }

        PluginManifest {
            plugin: crate::manifest::PluginSection {
                name: name.to_string(),
                version: version.to_string(),
                description: String::new(),
                author: String::new(),
                license: String::new(),
                entry_point: String::new(),
            },
            capabilities: Default::default(),
            dependencies,
        }
    }

    #[test]
    fn resolve_no_dependencies() {
        let resolver = DependencyResolver::new();
        let a = make_manifest("alpha", "1.0.0", &[]);
        let b = make_manifest("beta", "2.0.0", &[]);

        let order = resolver.resolve(&[a, b]).expect("should resolve");
        assert_eq!(order.len(), 2);
        assert_eq!(order[0].name, "alpha");
        assert_eq!(order[1].name, "beta");
    }

    #[test]
    fn resolve_linear_chain() {
        let resolver = DependencyResolver::new();
        let a = make_manifest("a", "1.0.0", &[]);
        let b = make_manifest("b", "1.0.0", &[("a", "^1.0")]);
        let c = make_manifest("c", "1.0.0", &[("b", "^1.0")]);

        let order = resolver.resolve(&[c, a, b]).expect("should resolve");
        let names: Vec<&str> = order.iter().map(|id| id.name.as_str()).collect();
        assert_eq!(names, vec!["a", "b", "c"]);
    }

    #[test]
    fn resolve_diamond_dependency() {
        let resolver = DependencyResolver::new();
        let a = make_manifest("a", "1.0.0", &[]);
        let b = make_manifest("b", "1.0.0", &[("a", "^1.0")]);
        let c = make_manifest("c", "1.0.0", &[("a", "^1.0")]);
        let d = make_manifest("d", "1.0.0", &[("b", "^1.0"), ("c", "^1.0")]);

        let order = resolver
            .resolve(&[d, b, c, a])
            .expect("should resolve");
        let names: Vec<&str> = order.iter().map(|id| id.name.as_str()).collect();

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
        let resolver = DependencyResolver::new();
        let a = make_manifest("a", "1.0.0", &[("b", "^1.0")]);
        let b = make_manifest("b", "1.0.0", &[("a", "^1.0")]);

        let result = resolver.resolve(&[a, b]);
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("cyclic"));
    }

    #[test]
    fn detect_three_node_cycle() {
        let resolver = DependencyResolver::new();
        let a = make_manifest("a", "1.0.0", &[("c", "^1.0")]);
        let b = make_manifest("b", "1.0.0", &[("a", "^1.0")]);
        let c = make_manifest("c", "1.0.0", &[("b", "^1.0")]);

        let result = resolver.resolve(&[a, b, c]);
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("cyclic"));
    }

    #[test]
    fn missing_dependency_error() {
        let resolver = DependencyResolver::new();
        let a = make_manifest("a", "1.0.0", &[("nonexistent", "^1.0")]);

        let result = resolver.resolve(&[a]);
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("not found"));
        assert!(msg.contains("nonexistent"));
    }

    #[test]
    fn version_conflict_error() {
        let resolver = DependencyResolver::new();
        let a = make_manifest("a", "1.0.0", &[]);
        let b = make_manifest("b", "1.0.0", &[("a", ">=2.0.0")]);

        let result = resolver.resolve(&[a, b]);
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("conflict"));
    }

    #[test]
    fn version_requirement_satisfied() {
        let resolver = DependencyResolver::new();
        let a = make_manifest("a", "1.2.3", &[]);
        let b = make_manifest("b", "1.0.0", &[("a", "^1.0")]);

        let order = resolver.resolve(&[a, b]).expect("should resolve");
        assert_eq!(order.len(), 2);
        assert_eq!(order[0].name, "a");
        assert_eq!(order[1].name, "b");
    }

    #[test]
    fn resolve_single_plugin_no_deps() {
        let resolver = DependencyResolver::new();
        let a = make_manifest("solo", "0.1.0", &[]);
        let order = resolver.resolve(&[a]).expect("should resolve");
        assert_eq!(order.len(), 1);
        assert_eq!(order[0].name, "solo");
    }

    #[test]
    fn resolve_empty_input() {
        let resolver = DependencyResolver::new();
        let order = resolver.resolve(&[]).expect("should resolve");
        assert!(order.is_empty());
    }

    #[test]
    fn self_cycle_detected() {
        let resolver = DependencyResolver::new();
        let a = make_manifest("a", "1.0.0", &[("a", "^1.0")]);

        let result = resolver.resolve(&[a]);
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("cyclic"));
    }

    #[test]
    fn is_compatible_checks() {
        let v = semver::Version::new(1, 5, 0);
        assert!(DependencyResolver::is_compatible(&v, "^1.0").expect("should parse"));
        assert!(DependencyResolver::is_compatible(&v, ">=1.0.0").expect("should parse"));
        assert!(!DependencyResolver::is_compatible(&v, ">=2.0.0").expect("should parse"));
        assert!(!DependencyResolver::is_compatible(&v, "^2.0").expect("should parse"));
    }

    #[test]
    fn is_compatible_invalid_req() {
        let v = semver::Version::new(1, 0, 0);
        let result = DependencyResolver::is_compatible(&v, "not valid");
        assert!(result.is_err());
    }
}
