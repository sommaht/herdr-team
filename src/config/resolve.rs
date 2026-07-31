//! Applying `base` chains to declared agents, and reporting what that made unusable.
//!
//! Pure. It is handed a merged declaration map and answers with the agents that resolved plus one
//! warning per agent that did not — the warnings stay data until `load` drains them into the sink,
//! which is what keeps every case here testable without one.
//!
//! Every failure is per-agent rather than per-map. A config with one broken entry keeps working for
//! every other entry, and the listing — the command a caller debugs with — still answers.

use std::collections::{BTreeMap, BTreeSet};

use super::{Agent, Declared};
use crate::harness;

// =====================================================================================================================
// Resolution
// =====================================================================================================================

/// What resolution produced.
pub struct Resolved {
    /// Every agent whose chain resolved, in name order.
    pub agents: BTreeMap<String, Agent>,
    /// One line per agent that did not, naming it and why.
    pub warnings: Vec<String>,
}

/// Applies every agent's base chain.
pub fn resolve(declared: &BTreeMap<String, Declared>) -> Resolved {
    let mut agents = BTreeMap::new();
    let mut warnings = Vec::new();

    for name in declared.keys() {
        match one(name, declared) {
            Ok(agent) => drop(agents.insert(name.clone(), agent)),
            Err(unusable) => warnings.push(unusable.warning(name)),
        }
    }

    Resolved { agents, warnings }
}

/// One agent, with its whole chain applied.
fn one(name: &str, declared: &BTreeMap<String, Declared>) -> Result<Agent, Unusable> {
    let chain = chain(name, declared)?;

    // Root first, so a child's value overwrites its base's and a child's args follow them. Appending
    // rather than replacing is what makes a child adding one flag a one-line agent: agent CLIs are
    // last-flag-wins, so nothing a child wants to change is lost by keeping the base's vector.
    let mut kind = None;
    let mut model = None;
    let mut effort = None;
    let mut prompt_file = None;
    let mut args = Vec::new();
    for link in &chain {
        kind = link.kind.clone().or(kind);
        model = link.model.clone().or(model);
        effort = link.effort.clone().or(effort);
        prompt_file = link.prompt_file.clone().or(prompt_file);
        args.extend(link.args.iter().cloned());
    }

    let kind = kind.ok_or(Unusable::NoKind)?;
    // `model` and `effort` exist only as whatever flags a harness spells them, and this build has
    // harnesses for two of herdr's kinds. An agent asking for them under any other is dropped rather
    // than started without them — a wrong answer nobody is told about is the failure this tool is
    // shaped to avoid.
    if (model.is_some() || effort.is_some()) && harness::by_kind(&kind).is_none() {
        return Err(Unusable::Untunable {
            kind,
            fields: fields_named(model.is_some(), effort.is_some()),
        });
    }

    Ok(Agent {
        kind,
        model,
        effort,
        args,
        prompt_file,
        // The agent's own declaration, not its base's: it is the file a reader goes to to change it.
        source: chain
            .last()
            .expect("a chain holds at least the agent itself")
            .source
            .clone(),
    })
}

/// The base chain, root first, ending with the agent itself.
///
/// # Errors
///
/// [`Unusable::DanglingBase`] when a `base` names an agent no layer declares, and [`Unusable::Cycle`]
/// when the walk returns to an agent it has already visited. The first lookup cannot dangle — the
/// caller iterates the map's own keys — so a failed lookup is always a `base`.
fn chain<'a>(name: &str, declared: &'a BTreeMap<String, Declared>) -> Result<Vec<&'a Declared>, Unusable> {
    let mut links = Vec::new();
    let mut seen = BTreeSet::new();
    let mut current = name.to_owned();

    loop {
        if !seen.insert(current.clone()) {
            return Err(Unusable::Cycle);
        }
        let link = declared.get(&current).ok_or(Unusable::DanglingBase { base: current })?;
        links.push(link);
        match &link.base {
            Some(base) => current = base.clone(),
            None => break,
        }
    }

    links.reverse();
    Ok(links)
}

// =====================================================================================================================
// Unusable Agents
// =====================================================================================================================

/// Why one agent did not resolve.
#[derive(Debug, PartialEq, Eq)]
enum Unusable {
    /// `base` names an agent no layer declares.
    DanglingBase {
        /// The name that was not found.
        base: String,
    },
    /// The chain returns to an agent it has already visited.
    Cycle,
    /// Nothing in the chain declares a kind.
    NoKind,
    /// The chain sets model or effort under a kind this build has no harness for.
    Untunable {
        /// That kind.
        kind: String,
        /// Which of the two fields were set.
        fields: String,
    },
}

impl Unusable {
    /// What the caller is told.
    ///
    /// Names the agent, the reason, and that the agent is gone — never a value out of the file.
    /// `model` and `effort` *become* the agent's command line, so the rule against logging an agent's
    /// arguments covers the fields that build one: the warning says which field was set, not what it
    /// was set to.
    fn warning(&self, name: &str) -> String {
        match self {
            Self::DanglingBase { base } => {
                format!("agent '{name}' bases on '{base}', which no config declares; '{name}' is unavailable")
            }
            Self::Cycle => format!("agent '{name}' is part of a base cycle; '{name}' is unavailable"),
            Self::NoKind => {
                format!("agent '{name}' declares no kind, and no base supplies one; '{name}' is unavailable")
            }
            Self::Untunable { kind, fields } => format!(
                "agent '{name}' sets {fields} under kind '{kind}', which this build can express only for {}; \
                 '{name}' is unavailable",
                harness::kinds().join(", ")
            ),
        }
    }
}

/// Which of the two tuning fields were set, for a warning to name.
fn fields_named(model: bool, effort: bool) -> String {
    match (model, effort) {
        (true, true) => "model and effort".to_owned(),
        (true, false) => "model".to_owned(),
        _ => "effort".to_owned(),
    }
}

// =====================================================================================================================
// Tests
// =====================================================================================================================

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    /// A declared agent, with only the fields a case is about.
    #[derive(Default)]
    struct Build {
        kind: Option<&'static str>,
        base: Option<&'static str>,
        model: Option<&'static str>,
        effort: Option<&'static str>,
        args: &'static [&'static str],
        prompt_file: Option<&'static str>,
    }

    impl Build {
        fn declared(self) -> Declared {
            Declared {
                kind: self.kind.map(ToOwned::to_owned),
                base: self.base.map(ToOwned::to_owned),
                model: self.model.map(ToOwned::to_owned),
                effort: self.effort.map(ToOwned::to_owned),
                args: self.args.iter().map(|argument| (*argument).to_owned()).collect(),
                prompt_file: self.prompt_file.map(PathBuf::from),
                source: PathBuf::from("/config.toml"),
            }
        }
    }

    /// A declaration map from `(name, agent)` pairs.
    fn map(agents: Vec<(&str, Build)>) -> BTreeMap<String, Declared> {
        agents
            .into_iter()
            .map(|(name, build)| (name.to_owned(), build.declared()))
            .collect()
    }

    #[test]
    fn an_agent_with_no_base_resolves_to_what_it_declared() {
        let declared = map(vec![(
            "cc",
            Build {
                kind: Some("claude"),
                args: &["--disallowed-tools", "AskUserQuestion"],
                ..Build::default()
            },
        )]);

        let resolved = resolve(&declared);

        assert!(resolved.warnings.is_empty());
        assert_eq!(resolved.agents["cc"].kind, "claude");
        assert_eq!(resolved.agents["cc"].args, ["--disallowed-tools", "AskUserQuestion"]);
    }

    #[test]
    fn a_child_inherits_kind_and_overrides_the_fields_it_states() {
        let declared = map(vec![
            (
                "cc",
                Build {
                    kind: Some("claude"),
                    model: Some("sonnet"),
                    ..Build::default()
                },
            ),
            (
                "opus",
                Build {
                    base: Some("cc"),
                    model: Some("opus"),
                    effort: Some("xhigh"),
                    ..Build::default()
                },
            ),
        ]);

        let resolved = resolve(&declared);

        let opus = &resolved.agents["opus"];
        assert_eq!(opus.kind, "claude", "inherited");
        assert_eq!(opus.model.as_deref(), Some("opus"), "the child wins");
        assert_eq!(opus.effort.as_deref(), Some("xhigh"));
    }

    /// The one field that appends rather than replacing, root first.
    #[test]
    fn args_are_concatenated_up_the_whole_chain_with_the_root_first() {
        let declared = map(vec![
            (
                "a",
                Build {
                    kind: Some("claude"),
                    args: &["--one"],
                    ..Build::default()
                },
            ),
            (
                "b",
                Build {
                    base: Some("a"),
                    args: &["--two"],
                    ..Build::default()
                },
            ),
            (
                "c",
                Build {
                    base: Some("b"),
                    args: &["--three"],
                    ..Build::default()
                },
            ),
        ]);

        let resolved = resolve(&declared);

        assert_eq!(resolved.agents["c"].args, ["--one", "--two", "--three"]);
    }

    #[test]
    fn a_prompt_file_is_inherited_and_a_child_may_replace_it() {
        let declared = map(vec![
            (
                "a",
                Build {
                    kind: Some("claude"),
                    prompt_file: Some("/config/a.md"),
                    ..Build::default()
                },
            ),
            ("b", Build { base: Some("a"), ..Build::default() }),
            (
                "c",
                Build {
                    base: Some("a"),
                    prompt_file: Some("/config/c.md"),
                    ..Build::default()
                },
            ),
        ]);

        let resolved = resolve(&declared);

        assert_eq!(
            resolved.agents["b"].prompt_file.as_deref(),
            Some(PathBuf::from("/config/a.md").as_path())
        );
        assert_eq!(
            resolved.agents["c"].prompt_file.as_deref(),
            Some(PathBuf::from("/config/c.md").as_path())
        );
    }

    #[test]
    fn a_base_no_config_declares_drops_that_agent_and_leaves_the_rest() {
        let declared = map(vec![
            (
                "cc",
                Build {
                    kind: Some("claude"),
                    ..Build::default()
                },
            ),
            (
                "broken",
                Build {
                    base: Some("typo"),
                    ..Build::default()
                },
            ),
        ]);

        let resolved = resolve(&declared);

        assert!(resolved.agents.contains_key("cc"), "the rest still works");
        assert!(!resolved.agents.contains_key("broken"));
        assert_eq!(
            resolved.warnings,
            ["agent 'broken' bases on 'typo', which no config declares; 'broken' is unavailable"]
        );
    }

    #[test]
    fn a_cycle_drops_every_agent_in_it() {
        let declared = map(vec![
            ("a", Build { base: Some("b"), ..Build::default() }),
            ("b", Build { base: Some("a"), ..Build::default() }),
            (
                "self",
                Build {
                    base: Some("self"),
                    ..Build::default()
                },
            ),
        ]);

        let resolved = resolve(&declared);

        assert!(resolved.agents.is_empty());
        assert_eq!(resolved.warnings.len(), 3);
        assert!(resolved.warnings.iter().all(|warning| warning.contains("base cycle")));
    }

    #[test]
    fn a_chain_that_never_declares_a_kind_is_unusable() {
        let declared = map(vec![
            ("a", Build::default()),
            (
                "b",
                Build {
                    base: Some("a"),
                    model: Some("opus"),
                    ..Build::default()
                },
            ),
        ]);

        let resolved = resolve(&declared);

        assert!(resolved.agents.is_empty());
        assert!(
            resolved
                .warnings
                .iter()
                .all(|warning| warning.contains("declares no kind")),
            "got {:?}",
            resolved.warnings
        );
    }

    /// The tuning fields need a harness, and this build has two.
    #[test]
    fn model_or_effort_under_a_kind_this_build_cannot_drive_drops_that_agent() {
        let declared = map(vec![(
            "gem",
            Build {
                kind: Some("gemini"),
                model: Some("pro"),
                ..Build::default()
            },
        )]);

        let resolved = resolve(&declared);

        assert!(resolved.agents.is_empty());
        let warning = &resolved.warnings[0];
        assert!(warning.contains("sets model under kind 'gemini'"), "got {warning}");
        assert!(warning.contains("claude, codex"), "got {warning}");
        assert!(
            !warning.contains("pro"),
            "the value is an argument and must not be logged"
        );
    }

    /// An unknown kind is only refused when it asks for something this build must spell.
    #[test]
    fn an_unknown_kind_that_asks_for_neither_is_passed_through_untouched() {
        let declared = map(vec![(
            "gem",
            Build {
                kind: Some("gemini"),
                args: &["--yolo"],
                ..Build::default()
            },
        )]);

        let resolved = resolve(&declared);

        assert!(resolved.warnings.is_empty());
        assert_eq!(resolved.agents["gem"].kind, "gemini");
        assert_eq!(resolved.agents["gem"].args, ["--yolo"]);
    }

    #[test]
    fn the_warning_names_whichever_of_the_two_fields_was_set() {
        assert_eq!(fields_named(true, true), "model and effort");
        assert_eq!(fields_named(true, false), "model");
        assert_eq!(fields_named(false, true), "effort");
    }
}
