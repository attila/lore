//! Predicate evaluator for the universal-pattern `applies_when` block.
//!
//! Two public entry points:
//!
//! * [`evaluate_applies_when`] — evaluates an [`AppliesWhen`] (parsed by U2
//!   in `src/chunking.rs`) against a [`CallContext`] (built by an adapter,
//!   see `src/engine/call_context.rs`). Total function — returns `bool`,
//!   never panics, never returns `Result`.
//! * [`command_matches_with_wrappers`] — the smart-prefix matcher used by
//!   the `bash_command_starts_with` branch. Walks past one `sudo` wrapper
//!   (with optional `-u USER`, short flags `-E`/`-H`) and one `env` wrapper
//!   (with `-i`, `-u VAR`, and `KEY=VAL` assignments) before checking the
//!   first non-wrapper token against the allowlist.
//!
//! The matcher operates on the raw command string — never through
//! `clean_terms` / `split_into_words` / FTS-cleaning — so short commands
//! like `gh` survive the 3-char filter. See
//! `docs/solutions/logic-errors/common-tool-commands-produce-zero-queryable-terms-2026-04-05.md`.
//!
//! Unimplemented variants (Track 1 documented limitations): nested env
//! wrappers (`env A=1 env B=2 cmd`), quoted commands inside `bash -c`. Both
//! are unusual in practice; the matcher returns `false` for them.

use crate::chunking::AppliesWhen;
use crate::engine::call_context::CallContext;

/// Tool-name string the `bash_command_starts_with` branch keys off.
const TOOL_BASH: &str = "Bash";

/// Evaluate an `AppliesWhen` predicate against a call context.
///
/// Semantics (R3 / R4 / R5):
///
/// * **AND across set keys**, **OR within each list**. A predicate with
///   both `tools` and `bash_command_starts_with` set passes only when both
///   branches pass.
/// * `tools`: when set, `ctx.tool_name` must equal one of the listed names
///   (case-sensitive). An empty list (`tools: []`) fails everything —
///   documented zero-element allowlist.
/// * `bash_command_starts_with`: when set, `ctx.tool_name` must equal
///   `"Bash"` AND `ctx.command` must start (after walking past wrappers)
///   with one of the listed tokens. An empty list fails everything.
/// * Neither key set: returns `true` unconditionally. Caller decides
///   whether to invoke the evaluator at all — Track 1 invokes it only for
///   universal chunks with a non-NULL `applies_when_json`.
pub fn evaluate_applies_when(predicate: &AppliesWhen, ctx: &CallContext) -> bool {
    if let Some(tools) = predicate.tools.as_ref()
        && !tools_match(tools, ctx)
    {
        return false;
    }
    if let Some(bash_prefixes) = predicate.bash_command_starts_with.as_ref()
        && !bash_prefix_match(bash_prefixes, ctx)
    {
        return false;
    }
    true
}

/// Returns `true` when the call's tool name is in the allowlist.
///
/// Empty allowlist → `false` (zero-element allowlist matches nothing).
/// Missing tool name → `false`.
fn tools_match(allowlist: &[String], ctx: &CallContext) -> bool {
    let Some(tool) = ctx.tool_name.as_deref() else {
        return false;
    };
    allowlist.iter().any(|t| t == tool)
}

/// Returns `true` when the call is a Bash call AND its command (after
/// wrapper-stripping) starts with one of the allowlisted tokens.
fn bash_prefix_match(allowlist: &[String], ctx: &CallContext) -> bool {
    if ctx.tool_name.as_deref() != Some(TOOL_BASH) {
        return false;
    }
    let Some(command) = ctx.command.as_deref() else {
        return false;
    };
    command_matches_with_wrappers(command, allowlist)
}

/// Walks past at most one `sudo` and one `env` wrapper in `command`,
/// returning `true` if the next non-wrapper token equals one of the
/// allowlisted strings.
///
/// **Sudo wrapper.** When the first token is `sudo`: advance past it, then
/// repeatedly consume short flags. Two flag shapes are recognised:
///
/// * `-u USER` — two-token unset-vs-run-as-user flag (consume both).
/// * `-E`, `-H` — single-token short flags (consume one).
///
/// Stop on the first token that is not one of those flag shapes.
///
/// **Env wrapper.** When the (now-current) token is `env`: advance past
/// it, then repeatedly consume:
///
/// * `-i` — single-token hermetic-environment flag.
/// * `-u VAR` — two-token unset-var flag.
/// * `KEY=VAL` — single-token assignment, where `KEY` matches
///   `[A-Z_][A-Z0-9_]*`.
///
/// Stop on the first token that does not match any of those shapes.
///
/// Operates on the raw command string. Empty command, command consisting
/// only of wrappers (no following token), and empty allowlist all return
/// `false`.
///
/// **Documented limitations** (Track 1): nested env wrappers
/// (`env A=1 env B=2 cmd`) and quoted commands inside `bash -c` are not
/// unwrapped. The matcher sees the literal token after one wrapper pass
/// and compares it against the allowlist as-is.
pub fn command_matches_with_wrappers(command: &str, allowlist: &[String]) -> bool {
    if allowlist.is_empty() {
        return false;
    }
    let mut tokens = command.split_whitespace();
    let Some(first) = tokens.next() else {
        return false;
    };

    let mut current = first;

    // Sudo wrapper.
    if current == "sudo" {
        let Some(next) = consume_sudo_flags(&mut tokens) else {
            return false;
        };
        current = next;
    }

    // Env wrapper.
    if current == "env" {
        let Some(next) = consume_env_args(&mut tokens) else {
            return false;
        };
        current = next;
    }

    allowlist.iter().any(|allowed| allowed == current)
}

/// Walks past sudo's flag arguments, returning the first non-flag token.
/// Returns `None` if the command ends inside the flag run.
///
/// Recognised:
/// * `-u USER` — two-token form. Both are consumed.
/// * `-E`, `-H` — single-token short flags. One token consumed each.
///
/// Any other token short-circuits and is returned as the next "real"
/// command head.
fn consume_sudo_flags<'a, I>(tokens: &mut I) -> Option<&'a str>
where
    I: Iterator<Item = &'a str>,
{
    loop {
        let token = tokens.next()?;
        match token {
            "-u" => {
                // Consume the user argument; abort if missing.
                tokens.next()?;
            }
            "-E" | "-H" => {
                // Single-token short flag — already consumed; loop.
            }
            _ => return Some(token),
        }
    }
}

/// Walks past env's argument run, returning the first non-arg token.
/// Returns `None` if the command ends inside the arg run.
///
/// Recognised:
/// * `-i` — hermetic-environment, single-token.
/// * `-u VAR` — unset-var, two-token form.
/// * `KEY=VAL` — single-token assignment with `[A-Z_][A-Z0-9_]*` key.
fn consume_env_args<'a, I>(tokens: &mut I) -> Option<&'a str>
where
    I: Iterator<Item = &'a str>,
{
    loop {
        let token = tokens.next()?;
        match token {
            "-i" => {}
            "-u" => {
                tokens.next()?;
            }
            other if is_env_assignment(other) => {}
            other => return Some(other),
        }
    }
}

/// Returns `true` if `token` matches `[A-Z_][A-Z0-9_]*=<value>` (the env
/// KEY=VAL assignment shape). The value side is unconstrained — anything
/// after the `=` counts.
fn is_env_assignment(token: &str) -> bool {
    let Some((key, _value)) = token.split_once('=') else {
        return false;
    };
    if key.is_empty() {
        return false;
    }
    let mut chars = key.chars();
    let first = chars.next().unwrap();
    if !(first.is_ascii_uppercase() || first == '_') {
        return false;
    }
    chars.all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
}

#[cfg(test)]
mod tests {
    use super::*;

    fn aw(tools: Option<&[&str]>, bash: Option<&[&str]>) -> AppliesWhen {
        AppliesWhen {
            tools: tools.map(|t| t.iter().map(|s| (*s).to_string()).collect()),
            bash_command_starts_with: bash.map(|t| t.iter().map(|s| (*s).to_string()).collect()),
        }
    }

    fn ctx_bash(command: &str) -> CallContext {
        CallContext {
            tool_name: Some(TOOL_BASH.to_string()),
            command: Some(command.to_string()),
            ..CallContext::empty()
        }
    }

    fn ctx_tool(name: &str) -> CallContext {
        CallContext {
            tool_name: Some(name.to_string()),
            ..CallContext::empty()
        }
    }

    // -----------------------------------------------------------------
    // Happy paths — bash_command_starts_with branch
    // -----------------------------------------------------------------

    #[test]
    fn bash_prefix_fires_on_simple_git_status() {
        let predicate = aw(None, Some(&["git"]));
        let ctx = ctx_bash("git status");
        assert!(evaluate_applies_when(&predicate, &ctx));
    }

    /// AE1: sudo wrapper walked past.
    #[test]
    fn bash_prefix_walks_past_sudo() {
        let predicate = aw(None, Some(&["git"]));
        let ctx = ctx_bash("sudo git status");
        assert!(evaluate_applies_when(&predicate, &ctx));
    }

    #[test]
    fn bash_prefix_walks_past_env_key_val() {
        let predicate = aw(None, Some(&["git"]));
        let ctx = ctx_bash("env GIT_PAGER=cat git log");
        assert!(evaluate_applies_when(&predicate, &ctx));
    }

    #[test]
    fn bash_prefix_or_within_list_matches_gh() {
        let predicate = aw(None, Some(&["git", "gh"]));
        let ctx = ctx_bash("gh pr create");
        assert!(evaluate_applies_when(&predicate, &ctx));
    }

    // -----------------------------------------------------------------
    // Happy / suppressed — tools branch (AE3)
    // -----------------------------------------------------------------

    #[test]
    fn tools_only_fires_for_listed_bash() {
        let predicate = aw(Some(&["Bash"]), None);
        let ctx = ctx_bash("ls");
        assert!(evaluate_applies_when(&predicate, &ctx));
    }

    #[test]
    fn tools_only_suppresses_non_listed_edit() {
        let predicate = aw(Some(&["Bash"]), None);
        let ctx = CallContext {
            tool_name: Some("Edit".to_string()),
            file_path: Some("foo.rs".to_string()),
            ..CallContext::empty()
        };
        assert!(!evaluate_applies_when(&predicate, &ctx));
    }

    // -----------------------------------------------------------------
    // AE4: AND across keys
    // -----------------------------------------------------------------

    #[test]
    fn both_keys_fire_on_matching_bash_git_push() {
        let predicate = aw(Some(&["Bash"]), Some(&["git", "gh"]));
        let ctx = ctx_bash("git push");
        assert!(evaluate_applies_when(&predicate, &ctx));
    }

    #[test]
    fn both_keys_suppress_bash_ls_when_prefix_mismatches() {
        let predicate = aw(Some(&["Bash"]), Some(&["git", "gh"]));
        let ctx = ctx_bash("ls");
        assert!(!evaluate_applies_when(&predicate, &ctx));
    }

    #[test]
    fn both_keys_suppress_edit_even_when_tool_missing_from_bash_list() {
        // Predicate requires Bash AND git/gh prefix; Edit fails the tool
        // check up front and never reaches the prefix check.
        let predicate = aw(Some(&["Bash"]), Some(&["git", "gh"]));
        let ctx = CallContext {
            tool_name: Some("Edit".to_string()),
            file_path: Some("foo.rs".to_string()),
            ..CallContext::empty()
        };
        assert!(!evaluate_applies_when(&predicate, &ctx));
    }

    // -----------------------------------------------------------------
    // AE2 (engine side): predicate suppresses unrelated Bash invocations
    // -----------------------------------------------------------------

    #[test]
    fn bash_prefix_suppresses_ls() {
        let predicate = aw(None, Some(&["git"]));
        let ctx = ctx_bash("ls");
        assert!(!evaluate_applies_when(&predicate, &ctx));
    }

    // -----------------------------------------------------------------
    // Empty-allowlist / empty-input edges
    // -----------------------------------------------------------------

    #[test]
    fn empty_bash_prefix_list_never_matches() {
        let predicate = aw(None, Some(&[]));
        let ctx = ctx_bash("git status");
        assert!(!evaluate_applies_when(&predicate, &ctx));
    }

    #[test]
    fn empty_tools_list_never_matches() {
        let predicate = aw(Some(&[]), None);
        let ctx = ctx_bash("git status");
        assert!(!evaluate_applies_when(&predicate, &ctx));
    }

    #[test]
    fn empty_command_string_no_match() {
        let predicate = aw(None, Some(&["git"]));
        let ctx = ctx_bash("");
        assert!(!evaluate_applies_when(&predicate, &ctx));
    }

    #[test]
    fn sudo_alone_no_match() {
        let predicate = aw(None, Some(&["git"]));
        let ctx = ctx_bash("sudo");
        assert!(!evaluate_applies_when(&predicate, &ctx));
    }

    // -----------------------------------------------------------------
    // Wrapper variants — happy paths
    // -----------------------------------------------------------------

    #[test]
    fn env_multiple_key_val_consumed() {
        let predicate = aw(None, Some(&["git"]));
        let ctx = ctx_bash("env A=1 B=2 git status");
        assert!(evaluate_applies_when(&predicate, &ctx));
    }

    #[test]
    fn env_dash_u_consumed() {
        let predicate = aw(None, Some(&["git"]));
        let ctx = ctx_bash("env -u VAR git status");
        assert!(evaluate_applies_when(&predicate, &ctx));
    }

    #[test]
    fn env_multiple_dash_u_consumed() {
        let predicate = aw(None, Some(&["git"]));
        let ctx = ctx_bash("env -u A -u B git status");
        assert!(evaluate_applies_when(&predicate, &ctx));
    }

    #[test]
    fn env_dash_i_consumed() {
        let predicate = aw(None, Some(&["git"]));
        let ctx = ctx_bash("env -i git status");
        assert!(evaluate_applies_when(&predicate, &ctx));
    }

    #[test]
    fn sudo_dash_u_user_consumed() {
        let predicate = aw(None, Some(&["git"]));
        let ctx = ctx_bash("sudo -u user git push");
        assert!(evaluate_applies_when(&predicate, &ctx));
    }

    #[test]
    fn sudo_dash_e_short_flag_consumed() {
        let predicate = aw(None, Some(&["git"]));
        let ctx = ctx_bash("sudo -E git push");
        assert!(evaluate_applies_when(&predicate, &ctx));
    }

    #[test]
    fn sudo_then_env_combined() {
        // sudo strips, then env strips, then git matches.
        let predicate = aw(None, Some(&["git"]));
        let ctx = ctx_bash("sudo env GIT_PAGER=cat git status");
        assert!(evaluate_applies_when(&predicate, &ctx));
    }

    // -----------------------------------------------------------------
    // Documented limitations — these MUST NOT fire in Track 1
    // -----------------------------------------------------------------

    /// Nested env wrappers: only the outer `env` is unwrapped; the next
    /// `env` is taken as the literal command head.
    #[test]
    fn limitation_nested_env_does_not_fire() {
        let predicate = aw(None, Some(&["git"]));
        let ctx = ctx_bash("env A=1 env B=2 git status");
        assert!(!evaluate_applies_when(&predicate, &ctx));
    }

    /// `bash -c "git status"`: matcher sees `bash` as the command head
    /// because quoted-command unwrapping is not implemented.
    #[test]
    fn limitation_bash_dash_c_does_not_fire() {
        let predicate = aw(None, Some(&["git"]));
        let ctx = ctx_bash("bash -c \"git status\"");
        assert!(!evaluate_applies_when(&predicate, &ctx));
    }

    // -----------------------------------------------------------------
    // Predicate with no set keys / fully-empty CallContext / totality
    // -----------------------------------------------------------------

    #[test]
    fn no_keys_set_returns_true_regardless_of_context() {
        let predicate = aw(None, None);
        // Empty context.
        assert!(evaluate_applies_when(&predicate, &CallContext::empty()));
        // Bash context.
        assert!(evaluate_applies_when(&predicate, &ctx_bash("anything")));
        // Edit context.
        assert!(evaluate_applies_when(&predicate, &ctx_tool("Edit")));
    }

    #[test]
    fn evaluator_is_total_on_empty_context() {
        // Predicate sets `tools`; ctx has no tool_name. Expect `false`,
        // no panic.
        let predicate = aw(Some(&["Bash"]), None);
        assert!(!evaluate_applies_when(&predicate, &CallContext::empty()));
    }

    #[test]
    fn evaluator_is_total_on_unicode_command() {
        let predicate = aw(None, Some(&["git"]));
        let ctx = ctx_bash("gît stâtus");
        // No panic; multi-byte head simply does not match `git`.
        assert!(!evaluate_applies_when(&predicate, &ctx));
    }

    #[test]
    fn evaluator_is_total_on_control_chars() {
        let predicate = aw(None, Some(&["git"]));
        // Control characters in command. Whitespace splitter sees `\t`
        // as whitespace; `\x07` (BEL) survives in the token.
        let ctx = ctx_bash("git\x07 status");
        // `git\x07` != `git`, so no match — but no panic.
        assert!(!evaluate_applies_when(&predicate, &ctx));
    }

    #[test]
    fn evaluator_is_total_on_non_bash_with_bash_prefix_predicate() {
        let predicate = aw(None, Some(&["git"]));
        let ctx = CallContext {
            tool_name: Some("Edit".to_string()),
            file_path: Some("foo.rs".to_string()),
            ..CallContext::empty()
        };
        assert!(!evaluate_applies_when(&predicate, &ctx));
    }

    // -----------------------------------------------------------------
    // is_env_assignment — direct unit coverage for the helper
    // -----------------------------------------------------------------

    #[test]
    fn env_assignment_detection() {
        assert!(is_env_assignment("FOO=bar"));
        assert!(is_env_assignment("_FOO=bar"));
        assert!(is_env_assignment("FOO_BAR=baz"));
        assert!(is_env_assignment("FOO123=bar"));
        assert!(is_env_assignment("FOO=")); // empty value is OK
        assert!(!is_env_assignment("foo=bar")); // lowercase
        assert!(!is_env_assignment("1FOO=bar")); // leading digit
        assert!(!is_env_assignment("FOO")); // no `=`
        assert!(!is_env_assignment("=bar")); // empty key
        assert!(!is_env_assignment("FOO-BAR=baz")); // dash not allowed
    }

    // -----------------------------------------------------------------
    // command_matches_with_wrappers — direct, allowlist-edge coverage
    // -----------------------------------------------------------------

    #[test]
    fn matcher_empty_allowlist_returns_false() {
        let allowlist: Vec<String> = vec![];
        assert!(!command_matches_with_wrappers("git status", &allowlist));
    }

    #[test]
    fn matcher_case_sensitive() {
        let allowlist = vec!["git".to_string()];
        assert!(!command_matches_with_wrappers("Git status", &allowlist));
        assert!(!command_matches_with_wrappers("GIT status", &allowlist));
    }

    #[test]
    fn matcher_only_first_token_compared() {
        let allowlist = vec!["status".to_string()];
        // Even though `status` appears in the command, it's not the head.
        assert!(!command_matches_with_wrappers("git status", &allowlist));
    }
}
