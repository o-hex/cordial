//! Working out what installing one plugin actually installs.
//!
//! Given an index and a list of things the user asked for, this produces an
//! ordered plan — dependencies before the plugins that need them — or refuses,
//! by name. It never fetches anything; resolution against an index somebody
//! hands you is entirely offline, and that is a property worth keeping, because
//! it is what makes every refusal below testable without a network.
//!
//! ## The order here is the start order in ADR-006
//!
//! [ADR-006](../../../docs/adr/ADR-006-plugin-events-and-first-party.md) already
//! establishes that plugins have a start order: `events.subscribe` is filtered
//! at subscribe time against types that have already been declared, so a
//! subscriber whose declarer has not started yet is refused rather than parked.
//! The order this module produces is the same order, and deliberately so —
//! dependencies first, each plugin appearing after everything it depends on.
//!
//! They are the same order because they answer the same question. Install order
//! exists because a plugin's dependency has to be on disk before the plugin is
//! any use; start order exists because a plugin's dependency has to be running
//! before the plugin is any use. Producing two orders from the same graph would
//! be two chances to disagree, and the disagreement would surface as a plugin
//! that installs cleanly and then fails to subscribe on every launch until
//! something unrelated changed the enumeration order.
//!
//! The one difference: [`Plan::pending`] filters out steps already installed at
//! the exact version, because those need no download. The *order* still
//! contains them — what is already on disk still has to start first.
//!
//! ## Capabilities are still the user's to grant
//!
//! Resolution and approval are two calls, not one, because a user cannot
//! approve a plan they have not been shown and the plan is what resolution
//! produces. [`resolve`] builds it; [`Plan::refuse_ungranted`] is what refuses
//! an install where some plugin in the plan asks for a capability the user has
//! not granted it. [`plan`] does both and is what an installer should call:
//! installing A must never quietly bring in B holding `assets.override`.

use crate::capability::Capability;
use crate::manifest::{Dependency, Requirement};
use crate::registry::{Entry, Index};
use semver::Version;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

/// Why an install will not happen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refusal {
    /// Nothing in the index publishes this id at all.
    Missing { id: String, needed_by: Option<String> },
    /// The index publishes it, but no version satisfies everything asked of it.
    Unsatisfiable { id: String, required: Vec<String>, available: Vec<Version> },
    /// The plugins depend on each other in a loop.
    Cycle { path: Vec<String> },
    /// Some plugin in the plan asks for capabilities the user has not granted
    /// it. `pulled_in_by` is set when it is not something the user asked for by
    /// name, which is the case this refusal mostly exists for.
    Ungranted { id: String, missing: Vec<Capability>, pulled_in_by: Option<String> },
    /// Resolution did not settle. Unreachable as the algorithm stands, and kept
    /// because an installer that hangs is worse than one that names a bug; if
    /// this is ever seen, the fixpoint loop below is wrong rather than the
    /// index being unusual.
    DidNotSettle,
}

impl fmt::Display for Refusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Refusal::Missing { id, needed_by: Some(by) } => {
                write!(f, "{by} needs {id}, which no index publishes")
            }
            Refusal::Missing { id, needed_by: None } => {
                write!(f, "no index publishes {id}")
            }
            Refusal::Unsatisfiable { id, required, available } => write!(
                f,
                "nothing published for {id} satisfies {}; available: {}",
                required.join(" and "),
                available.iter().map(|v| v.to_string()).collect::<Vec<_>>().join(", ")
            ),
            Refusal::Cycle { path } => {
                write!(f, "these plugins depend on each other in a loop: {}", path.join(" -> "))
            }
            Refusal::Ungranted { id, missing, pulled_in_by: Some(by) } => write!(
                f,
                "installing {by} would also install {id}, which asks for {} — not granted",
                names(missing)
            ),
            Refusal::Ungranted { id, missing, pulled_in_by: None } => {
                write!(f, "{id} asks for {} — not granted", names(missing))
            }
            Refusal::DidNotSettle => f.write_str("dependency resolution did not settle"),
        }
    }
}

fn names(caps: &[Capability]) -> String {
    caps.iter().map(|c| c.name()).collect::<Vec<_>>().join(", ")
}

/// What to install, in the order to install it.
#[derive(Debug, Clone)]
pub struct Plan<'a> {
    /// Dependencies first. Also the order to start them in; see the module note.
    pub steps: Vec<&'a Entry>,
    /// The ids the caller asked for by name, as against the ones pulled in.
    pub roots: BTreeSet<String>,
}

impl<'a> Plan<'a> {
    /// The steps that are not already installed at exactly this version.
    ///
    /// Exactly, not "at least" — a plugin already present at a newer version
    /// than the plan chose is a disagreement about what is installed, and
    /// answering it here by quietly keeping the newer one would mean the plan
    /// the user approved is not the plan that ran.
    pub fn pending(&self, installed: &BTreeMap<String, Version>) -> Vec<&'a Entry> {
        self.steps
            .iter()
            .copied()
            .filter(|e| installed.get(&e.id) != Some(&e.version))
            .collect()
    }

    /// Every capability the whole plan asks for, per plugin.
    pub fn capabilities(&self) -> BTreeMap<String, BTreeSet<Capability>> {
        self.steps.iter().map(|e| (e.id.clone(), e.capabilities.clone())).collect()
    }

    /// Which plugin in the plan pulled `id` in, if it was not asked for by name.
    fn pulled_in_by(&self, id: &str) -> Option<String> {
        if self.roots.contains(id) {
            return None;
        }
        self.steps.iter().find(|e| e.dependencies.iter().any(|d| d.id == id)).map(|e| e.id.clone())
    }

    /// Refuse the plan if anything in it asks for something it was not granted.
    ///
    /// `granted` is what the user has approved for this profile, including
    /// whatever they have just approved in the dialog that produced this plan.
    /// Nothing here widens it: a dependency's capabilities are the user's to
    /// grant, and a plugin arriving as somebody else's dependency is the case
    /// where that is easiest to lose sight of.
    pub fn refuse_ungranted(
        &self,
        granted: &BTreeMap<String, BTreeSet<Capability>>,
    ) -> Result<(), Refusal> {
        let empty = BTreeSet::new();
        for step in &self.steps {
            let held = granted.get(&step.id).unwrap_or(&empty);
            let missing: Vec<Capability> = step.capabilities.difference(held).copied().collect();
            if !missing.is_empty() {
                return Err(Refusal::Ungranted {
                    id: step.id.clone(),
                    missing,
                    pulled_in_by: self.pulled_in_by(&step.id),
                });
            }
        }
        Ok(())
    }
}

/// Resolve `wanted` against `index`, and refuse anything the user has not
/// granted. This is the entry point an installer wants.
pub fn plan<'a>(
    index: &'a Index,
    wanted: &[Dependency],
    granted: &BTreeMap<String, BTreeSet<Capability>>,
) -> Result<Plan<'a>, Refusal> {
    let plan = resolve(index, wanted)?;
    plan.refuse_ungranted(granted)?;
    Ok(plan)
}

/// Resolve `wanted` against `index` without consulting grants, so a UI can show
/// the user what an install would contain before asking them to approve it.
pub fn resolve<'a>(index: &'a Index, wanted: &[Dependency]) -> Result<Plan<'a>, Refusal> {
    let mut reqs: BTreeMap<String, Vec<(Option<String>, Requirement)>> = BTreeMap::new();
    for d in wanted {
        reqs.entry(d.id.clone()).or_default().push((None, d.req.clone()));
    }
    let roots: BTreeSet<String> = wanted.iter().map(|d| d.id.clone()).collect();

    // Requirements only ever accumulate, and adding one can only ever remove
    // candidates, so the version chosen for an id never goes up and the loop
    // settles. `DidNotSettle` is a bound on a proof rather than a case: a
    // resolver that spins is an installer that hangs with nothing on screen,
    // and this project has a list of afternoons lost to exactly that shape.
    let bound = index.entries.len().saturating_mul(index.entries.len()) + wanted.len() + 8;
    let mut chosen: BTreeMap<String, &Entry> = BTreeMap::new();
    let mut settled = false;
    for _ in 0..bound {
        let mut changed = false;
        for id in reqs.keys().cloned().collect::<Vec<_>>() {
            let rs = &reqs[&id];
            let held: Vec<&Requirement> = rs.iter().map(|(_, r)| r).collect();
            let Some(best) = index.best_match(&id, &held) else {
                return Err(if index.offers(&id).next().is_some() {
                    Refusal::Unsatisfiable {
                        id: id.clone(),
                        required: rs
                            .iter()
                            .map(|(by, r)| match by {
                                Some(by) => format!("{by}'s {r}"),
                                None => r.to_string(),
                            })
                            .collect(),
                        available: index.offers(&id).map(|e| e.version.clone()).collect(),
                    }
                } else {
                    Refusal::Missing {
                        id: id.clone(),
                        needed_by: rs.iter().find_map(|(by, _)| by.clone()),
                    }
                });
            };
            if chosen.get(&id).map(|e| &e.version) != Some(&best.version) {
                chosen.insert(id.clone(), best);
                changed = true;
            }
        }
        for (id, entry) in chosen.clone() {
            for d in &entry.dependencies {
                let held = reqs.entry(d.id.clone()).or_default();
                let mine = (Some(id.clone()), d.req.clone());
                if !held.contains(&mine) {
                    held.push(mine);
                    changed = true;
                }
            }
        }
        if !changed {
            settled = true;
            break;
        }
    }
    if !settled {
        return Err(Refusal::DidNotSettle);
    }

    order(&chosen, &roots).map(|steps| Plan { steps, roots })
}

/// Depth-first post-order, which is dependencies-first and finds cycles in the
/// same pass. Sorted at every branch so two runs over one index agree.
fn order<'a>(
    chosen: &BTreeMap<String, &'a Entry>,
    roots: &BTreeSet<String>,
) -> Result<Vec<&'a Entry>, Refusal> {
    let mut marks: BTreeMap<&str, Mark> = BTreeMap::new();
    let mut out: Vec<&'a Entry> = Vec::new();
    // Roots first so a plan reads in the order the user asked for things, then
    // everything else, so a plugin reachable only from a cycle is still visited
    // and still reported.
    let start: Vec<&str> =
        roots.iter().map(|s| s.as_str()).chain(chosen.keys().map(|s| s.as_str())).collect();
    for id in start {
        visit(id, chosen, &mut marks, &mut Vec::new(), &mut out)?;
    }
    Ok(out)
}

#[derive(Clone, Copy, PartialEq)]
enum Mark {
    Open,
    Done,
}

fn visit<'a>(
    id: &str,
    chosen: &BTreeMap<String, &'a Entry>,
    marks: &mut BTreeMap<&'a str, Mark>,
    stack: &mut Vec<String>,
    out: &mut Vec<&'a Entry>,
) -> Result<(), Refusal> {
    let Some(entry) = chosen.get(id) else {
        return Ok(());
    };
    match marks.get(id) {
        Some(Mark::Done) => return Ok(()),
        Some(Mark::Open) => {
            // Reported from where the loop closes, so the message names every
            // plugin in it rather than only the one we happened to start from.
            let from = stack.iter().position(|s| s.as_str() == id).unwrap_or(0);
            let mut path: Vec<String> = stack[from..].to_vec();
            path.push(id.to_string());
            return Err(Refusal::Cycle { path });
        }
        None => {}
    }
    marks.insert(entry.id.as_str(), Mark::Open);
    stack.push(entry.id.clone());
    for d in &entry.dependencies {
        visit(&d.id, chosen, marks, stack, out)?;
    }
    stack.pop();
    marks.insert(entry.id.as_str(), Mark::Done);
    out.push(*entry);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::Index;

    /// Build an index without repeating a url and a hash for every entry. The
    /// hash is derived from the id and version so two entries never collide,
    /// and nothing in this module looks at either field.
    fn index(entries: &[(&str, &str, &[&str], &[(&str, &str)])]) -> Index {
        let mut plugins = Vec::new();
        for (n, (id, version, caps, deps)) in entries.iter().enumerate() {
            let caps = caps.iter().map(|c| format!("\"{c}\"")).collect::<Vec<_>>().join(",");
            let deps = deps
                .iter()
                .map(|(d, r)| format!("\"{d}\":\"{r}\""))
                .collect::<Vec<_>>()
                .join(",");
            plugins.push(format!(
                r#"{{"id":"{id}","version":"{version}","capabilities":[{caps}],
                    "dependencies":{{{deps}}},
                    "url":"https://x.invalid/{id}-{version}.tar.zst",
                    "hash":"sha256:{:064x}"}}"#,
                n + 1
            ));
        }
        Index::parse_unverified(&format!(r#"{{"format":1,"plugins":[{}]}}"#, plugins.join(",")))
            .unwrap()
    }

    fn want(id: &str, req: &str) -> Vec<Dependency> {
        vec![Dependency::new(id, req).unwrap()]
    }

    fn ids(plan: &Plan<'_>) -> Vec<String> {
        plan.steps.iter().map(|e| e.id.clone()).collect()
    }

    fn granting(pairs: &[(&str, &[&str])]) -> BTreeMap<String, BTreeSet<Capability>> {
        pairs
            .iter()
            .map(|(id, caps)| {
                (id.to_string(), caps.iter().map(|c| Capability::parse(c).unwrap()).collect())
            })
            .collect()
    }

    #[test]
    fn a_dependency_is_installed_before_the_plugin_that_needs_it() {
        // Which is also the order they have to start in: a subscriber whose
        // declarer has not started yet is refused at subscribe time (ADR-006).
        let i = index(&[
            ("app", "1.0.0", &[], &[("lib", "^1.0.0")]),
            ("lib", "1.2.0", &[], &[]),
        ]);
        let p = resolve(&i, &want("app", "^1.0.0")).unwrap();
        assert_eq!(ids(&p), ["lib", "app"]);
    }

    #[test]
    fn a_missing_dependency_is_named_rather_than_skipped() {
        let i = index(&[("app", "1.0.0", &[], &[("lib", "^1.0.0")])]);
        let e = resolve(&i, &want("app", "^1.0.0")).unwrap_err();
        assert_eq!(
            e,
            Refusal::Missing { id: "lib".into(), needed_by: Some("app".into()) },
            "{e}"
        );
    }

    #[test]
    fn an_unsatisfiable_constraint_is_refused_with_what_was_on_offer() {
        let i = index(&[
            ("app", "1.0.0", &[], &[("lib", "^2.0.0")]),
            ("lib", "1.2.0", &[], &[]),
        ]);
        let e = resolve(&i, &want("app", "^1.0.0")).unwrap_err();
        match e {
            Refusal::Unsatisfiable { id, available, .. } => {
                assert_eq!(id, "lib");
                assert_eq!(available, [Version::new(1, 2, 0)]);
            }
            other => panic!("expected an unsatisfiable constraint, got {other}"),
        }
    }

    #[test]
    fn two_dependents_wanting_incompatible_versions_are_refused_not_installed_twice() {
        // A plugin lives at `plugins/<id>/`, so there is one copy of it and
        // exactly one version can win. Choosing one silently would leave the
        // other dependent running against a version it said it could not use.
        let i = index(&[
            ("app", "1.0.0", &[], &[("one", "^1.0.0"), ("two", "^1.0.0")]),
            ("one", "1.0.0", &[], &[("lib", "^1.0.0")]),
            ("two", "1.0.0", &[], &[("lib", "^2.0.0")]),
            ("lib", "1.0.0", &[], &[]),
            ("lib", "2.0.0", &[], &[]),
        ]);
        let e = resolve(&i, &want("app", "^1.0.0")).unwrap_err();
        assert!(matches!(e, Refusal::Unsatisfiable { ref id, .. } if id == "lib"), "{e}");
    }

    #[test]
    fn a_cycle_is_refused_and_the_loop_is_named() {
        let i = index(&[
            ("a", "1.0.0", &[], &[("b", "^1.0.0")]),
            ("b", "1.0.0", &[], &[("a", "^1.0.0")]),
        ]);
        let e = resolve(&i, &want("a", "^1.0.0")).unwrap_err();
        match e {
            Refusal::Cycle { path } => {
                assert!(path.contains(&"a".to_string()) && path.contains(&"b".to_string()));
            }
            other => panic!("expected a cycle, got {other}"),
        }
    }

    #[test]
    fn a_plugin_depending_on_itself_is_a_cycle_rather_than_a_hang() {
        let i = index(&[("a", "1.0.0", &[], &[("a", "^1.0.0")])]);
        assert!(matches!(resolve(&i, &want("a", "^1.0.0")), Err(Refusal::Cycle { .. })));
    }

    #[test]
    fn a_dependency_cannot_smuggle_in_a_capability_the_user_did_not_grant() {
        // The refusal this whole module exists for. Approving a plugin that
        // wants `log` must not be a way to end up with one holding
        // `assets.override`, and the user is never shown the second decision
        // if resolution does not make it one.
        let i = index(&[
            ("app", "1.0.0", &["log"], &[("sneaky", "^1.0.0")]),
            ("sneaky", "1.0.0", &["assets.override"], &[]),
        ]);
        let granted = granting(&[("app", &["log"])]);
        let e = plan(&i, &want("app", "^1.0.0"), &granted).unwrap_err();
        assert_eq!(
            e,
            Refusal::Ungranted {
                id: "sneaky".into(),
                missing: vec![Capability::AssetsOverride],
                pulled_in_by: Some("app".into()),
            },
            "{e}"
        );
        // And it installs once that capability is granted to it by name.
        let granted = granting(&[("app", &["log"]), ("sneaky", &["assets.override"])]);
        assert_eq!(ids(&plan(&i, &want("app", "^1.0.0"), &granted).unwrap()), ["sneaky", "app"]);
    }

    #[test]
    fn resolution_without_grants_still_reports_the_whole_plan() {
        // Because a user cannot approve a plan they have not been shown, and
        // the plan is what resolution produces.
        let i = index(&[
            ("app", "1.0.0", &["log"], &[("sneaky", "^1.0.0")]),
            ("sneaky", "1.0.0", &["assets.override"], &[]),
        ]);
        let p = resolve(&i, &want("app", "^1.0.0")).unwrap();
        assert_eq!(ids(&p), ["sneaky", "app"]);
        assert!(p.capabilities()["sneaky"].contains(&Capability::AssetsOverride));
        assert!(p.refuse_ungranted(&granting(&[("app", &["log"])])).is_err());
    }

    #[test]
    fn what_is_already_installed_is_not_downloaded_again_but_still_starts_first() {
        let i = index(&[
            ("app", "1.0.0", &[], &[("lib", "^1.0.0")]),
            ("lib", "1.2.0", &[], &[]),
        ]);
        let p = resolve(&i, &want("app", "^1.0.0")).unwrap();
        let installed: BTreeMap<String, Version> =
            [("lib".to_string(), Version::new(1, 2, 0))].into_iter().collect();
        assert_eq!(
            p.pending(&installed).iter().map(|e| e.id.clone()).collect::<Vec<_>>(),
            ["app"]
        );
        assert_eq!(ids(&p), ["lib", "app"], "the start order still contains it");
    }

    #[test]
    fn an_installed_version_the_plan_did_not_choose_is_still_pending() {
        // Keeping a newer copy quietly would mean the plan that ran is not the
        // plan the user approved.
        let i = index(&[("lib", "1.0.0", &[], &[])]);
        let p = resolve(&i, &want("lib", "=1.0.0")).unwrap();
        let installed: BTreeMap<String, Version> =
            [("lib".to_string(), Version::new(2, 0, 0))].into_iter().collect();
        assert_eq!(p.pending(&installed).len(), 1);
    }

    #[test]
    fn a_diamond_resolves_to_one_shared_copy() {
        // ADR-006 is explicit that a dependency is "resolved once, shared, not
        // restarted per dependent"; the plan has to say the same.
        let i = index(&[
            ("app", "1.0.0", &[], &[("one", "^1.0.0"), ("two", "^1.0.0")]),
            ("one", "1.0.0", &[], &[("lib", "^1.0.0")]),
            ("two", "1.0.0", &[], &[("lib", "^1.2.0")]),
            ("lib", "1.0.0", &[], &[]),
            ("lib", "1.4.0", &[], &[]),
        ]);
        let p = resolve(&i, &want("app", "^1.0.0")).unwrap();
        assert_eq!(ids(&p), ["lib", "one", "two", "app"]);
        assert_eq!(p.steps[0].version, Version::new(1, 4, 0));
    }
}
