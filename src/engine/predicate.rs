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
//!   (with optional `-u USER`, short flags `-E`/`-H`), repeats over any
//!   number of `env` wrappers (each with `-i`, `-u VAR`, and `KEY=VAL`
//!   assignments), then optionally unwraps a `bash -c "..."` /
//!   `sh -c '...'` quoted command before checking the first non-wrapper
//!   token against the allowlist.
//!
//! The matcher operates on the raw command string — never through
//! `clean_terms` / `split_into_words` / FTS-cleaning — so short commands
//! like `gh` survive the 3-char filter. See
//! `docs/solutions/logic-errors/common-tool-commands-produce-zero-queryable-terms-2026-04-05.md`.
//!
//! Unimplemented variants (Track 1 documented limitations): nested-quote
//! / escaped-quote handling inside `bash -c` (e.g.
//! `bash -c "echo \"git status\""`), quoted KEY=VAL with internal spaces,
//! and recursive wrapper-stripping inside the `bash -c` quoted body
//! (`bash -c "sudo git status"` does not unwrap the inner `sudo`). Both
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

/// Walks past at most one `sudo`, any number of nested `env` wrappers,
/// and one optional `bash -c "..."` / `sh -c '...'` quoted-command
/// wrapper in `command`, returning `true` if the resulting effective
/// command head equals one of the allowlisted strings.
///
/// **Leading whitespace.** The command is implicitly trimmed because
/// tokenisation goes through [`str::split_whitespace`], which discards
/// leading and inter-token whitespace.
///
/// **Sudo wrapper.** When the first token is `sudo`: advance past it, then
/// repeatedly consume short flags. Two flag shapes are recognised:
///
/// * `-u USER` — two-token unset-vs-run-as-user flag (consume both).
/// * `-E`, `-H` — single-token short flags (consume one).
///
/// Stop on the first token that is not one of those flag shapes.
///
/// **Env wrapper (repeated).** While the next token is `env`: advance
/// past it, then repeatedly consume:
///
/// * `-i` — single-token hermetic-environment flag.
/// * `-u VAR` — two-token unset-var flag.
/// * `KEY=VAL` — single-token assignment, where `KEY` matches
///   `[A-Z_][A-Z0-9_]*`.
///
/// Stop on the first token that does not match any of those shapes; loop
/// back to env-detection for nested forms like
/// `env A=1 env B=2 git status`.
///
/// **`bash -c "..."` / `sh -c '...'` wrapper.** When the next token is
/// `bash` or `sh` followed by `-c` followed by a quoted string, extract
/// the first whitespace-delimited token from inside the quoted string
/// and use that as the effective command head. Both `"..."` and `'...'`
/// quote forms are accepted. If the `-c` argument is unquoted, the
/// remainder of the original command is treated as the body and the
/// first whitespace-delimited token is used. Empty or whitespace-only
/// quoted bodies fail.
///
/// Operates on the raw command string. Empty command, command consisting
/// only of wrappers (no following token), and empty allowlist all return
/// `false`.
///
/// **Documented limitations** (Track 1): nested-quote / escaped-quote
/// handling inside `bash -c` (e.g. `bash -c "echo \"git status\""`),
/// quoted `KEY=VAL` with internal spaces, and recursive wrapper-stripping
/// inside the `bash -c` quoted body (the body's first token is taken
/// verbatim — `bash -c "sudo git status"` resolves to `sudo`, not `git`).
pub fn command_matches_with_wrappers(command: &str, allowlist: &[String]) -> bool {
    if allowlist.is_empty() {
        return false;
    }
    let Some(effective) = effective_command_head(command) else {
        return false;
    };
    allowlist.iter().any(|allowed| allowed == &effective)
}

/// Walks the wrapper sequence and returns the first non-wrapper token, or
/// `None` if the command is exhausted before producing one or the
/// `bash -c` body is empty / whitespace-only. Returns owned `String`
/// because the `bash -c` branch may slice the body of the original
/// command and produce a fresh substring.
fn effective_command_head(command: &str) -> Option<String> {
    // Use match_indices so we keep byte offsets into the original string;
    // the bash -c handler needs to slice the unquoted-then-requoted body
    // out of the original command.
    let mut tokens = command.split_whitespace().peekable();
    let mut current = tokens.next()?;

    // Sudo wrapper — at most one outer scope.
    if current == "sudo" {
        current = consume_sudo_flags(&mut tokens)?;
    }

    // Env wrappers — repeat for nested `env A=1 env B=2 cmd` forms.
    while current == "env" {
        current = consume_env_args(&mut tokens)?;
    }

    // bash -c "..." / sh -c '...' — at most one wrapper. Scan the raw
    // string for the standalone `-c` token to recover the substring
    // after it without losing quote boundaries (split_whitespace would
    // shred `"git status"` into two tokens).
    if (current == "bash" || current == "sh") && tokens.peek() == Some(&"-c") {
        let body = bash_c_body(command)?;
        return first_token_of_quoted_body(body);
    }

    Some(current.to_string())
}

/// Returns the substring of `command` that follows the `-c` argument of
/// the trailing `bash`/`sh` invocation. The caller has already verified
/// (via `split_whitespace`) that a `-c` token exists; this helper walks
/// the original string to find that token's byte position so the
/// returned slice preserves quote boundaries.
fn bash_c_body(command: &str) -> Option<&str> {
    // Walk byte-by-byte looking for the standalone `-c` token. We accept
    // any whitespace before/after, and require it not be part of a longer
    // word like `-cd`. The first such occurrence belongs to the bash -c
    // wrapper because the caller's tokeniser already validated it.
    let bytes = command.as_bytes();
    let mut i = 0;
    while i + 1 < bytes.len() {
        if bytes[i] == b'-' && bytes[i + 1] == b'c' {
            let before_ok = i == 0 || (bytes[i - 1] as char).is_whitespace();
            let after_ok = i + 2 == bytes.len() || (bytes[i + 2] as char).is_whitespace();
            if before_ok && after_ok {
                let body = command[i + 2..].trim_start();
                if body.is_empty() {
                    return None;
                }
                return Some(body);
            }
        }
        i += 1;
    }
    None
}

/// Given the body of a `bash -c` invocation (everything after `-c`),
/// returns the first whitespace-delimited token of the underlying
/// command. Handles both single- and double-quoted forms; an unquoted
/// body falls back to plain whitespace tokenisation.
///
/// Empty and whitespace-only quoted bodies return `None`. Nested-quote
/// and escaped-quote handling is out of scope (Track 1 documented
/// limitation): the first occurrence of the matching outer quote ends
/// the body.
fn first_token_of_quoted_body(body: &str) -> Option<String> {
    let body = body.trim_start();
    let first = body.chars().next()?;
    let inner = if first == '"' || first == '\'' {
        let rest = &body[first.len_utf8()..];
        let end = rest.find(first)?;
        &rest[..end]
    } else {
        body
    };
    inner.split_whitespace().next().map(str::to_string)
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
    // Leading whitespace — split_whitespace silently discards it
    // -----------------------------------------------------------------

    #[test]
    fn leading_whitespace_is_trimmed() {
        let predicate = aw(None, Some(&["git"]));
        let ctx = ctx_bash("   git status");
        assert!(evaluate_applies_when(&predicate, &ctx));
    }

    #[test]
    fn leading_tabs_and_spaces_trimmed() {
        let predicate = aw(None, Some(&["git"]));
        let ctx = ctx_bash("\t  \tgit status");
        assert!(evaluate_applies_when(&predicate, &ctx));
    }

    // -----------------------------------------------------------------
    // Nested env wrappers — `env A=1 env B=2 git status` now fires
    // -----------------------------------------------------------------

    /// Nested env wrappers are unwrapped repeatedly; both `env` scopes
    /// are consumed before the matcher checks the head.
    #[test]
    fn nested_env_wrappers_fire() {
        let predicate = aw(None, Some(&["git"]));
        let ctx = ctx_bash("env A=1 env B=2 git status");
        assert!(evaluate_applies_when(&predicate, &ctx));
    }

    #[test]
    fn nested_env_wrappers_with_mixed_flags_fire() {
        let predicate = aw(None, Some(&["git"]));
        let ctx = ctx_bash("env A=1 env -u VAR -i KEY=val git status");
        assert!(evaluate_applies_when(&predicate, &ctx));
    }

    #[test]
    fn triple_nested_env_wrappers_fire() {
        let predicate = aw(None, Some(&["git"]));
        let ctx = ctx_bash("env A=1 env B=2 env C=3 git status");
        assert!(evaluate_applies_when(&predicate, &ctx));
    }

    // -----------------------------------------------------------------
    // bash -c "..." / sh -c '...' — quoted-command extraction
    // -----------------------------------------------------------------

    #[test]
    fn bash_dash_c_double_quoted_fires() {
        let predicate = aw(None, Some(&["git"]));
        let ctx = ctx_bash("bash -c \"git status\"");
        assert!(evaluate_applies_when(&predicate, &ctx));
    }

    #[test]
    fn sh_dash_c_single_quoted_fires() {
        let predicate = aw(None, Some(&["gh"]));
        let ctx = ctx_bash("sh -c 'gh pr create'");
        assert!(evaluate_applies_when(&predicate, &ctx));
    }

    #[test]
    fn bash_dash_c_does_not_match_unrelated_command() {
        let predicate = aw(None, Some(&["git"]));
        let ctx = ctx_bash("bash -c \"echo hello\"");
        assert!(!evaluate_applies_when(&predicate, &ctx));
    }

    #[test]
    fn bash_dash_c_unquoted_extracts_first_token() {
        // Track 1 fallback: when -c body is unquoted, take the first
        // whitespace-delimited token.
        let predicate = aw(None, Some(&["git"]));
        let ctx = ctx_bash("bash -c git");
        assert!(evaluate_applies_when(&predicate, &ctx));
    }

    #[test]
    fn bash_dash_c_empty_double_quotes_no_match() {
        let predicate = aw(None, Some(&["git"]));
        let ctx = ctx_bash("bash -c \"\"");
        assert!(!evaluate_applies_when(&predicate, &ctx));
    }

    #[test]
    fn bash_dash_c_whitespace_only_quoted_no_match() {
        let predicate = aw(None, Some(&["git"]));
        let ctx = ctx_bash("bash -c \"   \"");
        assert!(!evaluate_applies_when(&predicate, &ctx));
    }

    #[test]
    fn bash_dash_c_no_following_arg_no_match() {
        let predicate = aw(None, Some(&["git"]));
        let ctx = ctx_bash("bash -c");
        assert!(!evaluate_applies_when(&predicate, &ctx));
    }

    /// `bash -c` body's first token is taken verbatim — wrapper-stripping
    /// inside the quoted body is a documented limitation. The body's
    /// first token is `sudo`, which does not match the allowlist.
    #[test]
    fn limitation_bash_dash_c_inner_sudo_not_unwrapped() {
        let predicate = aw(None, Some(&["git"]));
        let ctx = ctx_bash("bash -c \"sudo git status\"");
        assert!(!evaluate_applies_when(&predicate, &ctx));
    }

    /// Escaped-quote / nested-quote handling inside `bash -c` is out of
    /// scope: the simple matcher splits at the first matching outer
    /// quote, which truncates `echo "git status"` after the inner `\"`.
    /// Documented Track 1 limitation.
    #[test]
    fn limitation_bash_dash_c_escaped_quotes_undefined() {
        let predicate = aw(None, Some(&["git"]));
        let ctx = ctx_bash("bash -c \"echo \\\"git status\\\"\"");
        // Document current behaviour: the simple matcher does not handle
        // backslash-escaped quotes — it just doesn't fire for this shape.
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
