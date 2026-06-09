// SPDX-License-Identifier: MIT OR Apache-2.0

//! Hook pipeline for OpenAI Codex CLI lifecycle events.
//!
//! Codex uses a Claude-compatible output envelope, but its stdin payloads
//! have Codex-specific fields (`turn_id`, `tool_use_id`, `prompt`) and, as
//! captured from the local CLI, Bash `PostToolUse.tool_response` is plain
//! text rather than Claude's structured exit-code object.

use std::collections::HashSet;
use std::io::Read as _;
use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::config::Config;
use crate::database::KnowledgeDB;
use crate::embeddings::Embedder;
use crate::engine::{self, CallContext};
use crate::hook::{self, HookOutput};
use crate::lore_debug;

/// Deserialized from Codex hook stdin JSON.
#[derive(Debug, Deserialize)]
pub struct CodexHookInput {
    pub hook_event_name: String,
    pub session_id: Option<String>,
    pub tool_name: Option<String>,
    pub tool_input: Option<serde_json::Value>,
    pub transcript_path: Option<String>,
    pub tool_response: Option<serde_json::Value>,
    pub prompt: Option<String>,
    pub source: Option<String>,
}

/// Read stdin and parse as [`CodexHookInput`].
pub fn read_input() -> anyhow::Result<CodexHookInput> {
    let mut buf = String::new();
    std::io::stdin().read_to_string(&mut buf)?;
    let input: CodexHookInput = serde_json::from_str(&buf)?;
    Ok(input)
}

/// Main Codex dispatcher. Returns `Some(HookOutput)` when context should be
/// injected, or `None` when the hook should produce no output.
pub fn handle_hook(
    input: &CodexHookInput,
    db: &KnowledgeDB,
    embedder: &dyn Embedder,
    config: &Config,
) -> anyhow::Result<Option<HookOutput>> {
    lore_debug!(
        "codex hook event={} session={} tool={}",
        input.hook_event_name,
        input.session_id.as_deref().unwrap_or("none"),
        input.tool_name.as_deref().unwrap_or("none"),
    );

    match input.hook_event_name.as_str() {
        "SessionStart" => handle_session_start(input, db, config),
        "PreToolUse" => handle_pre_tool_use(input, db, embedder, config),
        "PostToolUse" => handle_post_tool_use(input, db, embedder, config),
        "UserPromptSubmit" => handle_user_prompt_submit(input, db, embedder, config),
        _ => {
            lore_debug!("unknown Codex event, producing no output");
            Ok(None)
        }
    }
}

fn handle_session_start(
    input: &CodexHookInput,
    db: &KnowledgeDB,
    config: &Config,
) -> anyhow::Result<Option<HookOutput>> {
    if let Some(source) = input.source.as_deref()
        && !matches!(source, "startup" | "compact" | "resume" | "clear")
    {
        eprintln!("lore codex-hook: unknown SessionStart source '{source}', treating as startup");
        lore_debug!("Codex SessionStart unknown source: {source}");
    }

    if matches!(
        input.source.as_deref(),
        None | Some("startup" | "compact" | "clear")
    ) && let Some(path) = input.session_dedup_path()
        && let Err(e) = hook::reset_dedup(&path)
    {
        eprintln!("lore codex-hook: failed to create dedup file: {e}");
        lore_debug!("Codex SessionStart dedup reset error: {e}");
    }

    let context = hook::format_session_context(db, &config.knowledge_dir)?;
    Ok(Some(HookOutput::additional_context(
        "SessionStart",
        context,
    )))
}

fn handle_pre_tool_use(
    input: &CodexHookInput,
    db: &KnowledgeDB,
    embedder: &dyn Embedder,
    config: &Config,
) -> anyhow::Result<Option<HookOutput>> {
    run_pre_context_pipeline(
        input,
        input.to_call_context(),
        "PreToolUse",
        db,
        embedder,
        config,
    )
}

fn handle_user_prompt_submit(
    input: &CodexHookInput,
    db: &KnowledgeDB,
    embedder: &dyn Embedder,
    config: &Config,
) -> anyhow::Result<Option<HookOutput>> {
    run_pre_context_pipeline(
        input,
        input.to_prompt_call_context(),
        "UserPromptSubmit",
        db,
        embedder,
        config,
    )
}

fn handle_post_tool_use(
    input: &CodexHookInput,
    db: &KnowledgeDB,
    embedder: &dyn Embedder,
    config: &Config,
) -> anyhow::Result<Option<HookOutput>> {
    if input.tool_name.as_deref() != Some("Bash") {
        return Ok(None);
    }
    let Some(text) = input.tool_response_text() else {
        return Ok(None);
    };
    if !looks_like_error_output(&text) {
        lore_debug!("Codex PostToolUse: Bash output did not look like an error, skipping");
        return Ok(None);
    }
    let Some(query) = engine::query_from_error_text(&text) else {
        return Ok(None);
    };

    lore_debug!("Codex PostToolUse: error query: {query}");
    let (results, _phases) = hook::search_with_threshold(db, embedder, config, &query)?;
    if results.is_empty() {
        return Ok(None);
    }

    Ok(Some(HookOutput::additional_context(
        "PostToolUse",
        hook::format_imperative(&results),
    )))
}

fn run_pre_context_pipeline(
    input: &CodexHookInput,
    cc: CallContext,
    output_event_name: &str,
    db: &KnowledgeDB,
    embedder: &dyn Embedder,
    config: &Config,
) -> anyhow::Result<Option<HookOutput>> {
    let Some((inferred_langs, cleaned_terms)) = engine::extract_query(&cc) else {
        lore_debug!("Codex {output_event_name}: no query extracted");
        return Ok(None);
    };
    let query = engine::assemble_fts_query(&inferred_langs, &cleaned_terms).unwrap_or_default();
    let (seeds, _phases) = hook::search_with_threshold_gated(
        db,
        embedder,
        config,
        &query,
        &cleaned_terms,
        &inferred_langs,
    )?;
    if seeds.is_empty() {
        return Ok(None);
    }

    let expanded = hook::expand_to_siblings(db, &seeds);
    let after_predicate = hook::apply_predicate_filter(expanded, &cc);
    if after_predicate.is_empty() {
        return Ok(None);
    }

    let combined = if let Some(path) = input.session_dedup_path()
        && path.exists()
    {
        match hook::dedup_filter_and_record(&path, &after_predicate) {
            Ok(filtered) => filtered,
            Err(e) => {
                eprintln!("lore codex-hook: dedup filter error: {e}");
                lore_debug!("Codex dedup filter error (continuing without dedup): {e}");
                after_predicate
            }
        }
    } else {
        after_predicate
    };

    if combined.is_empty() {
        return Ok(None);
    }

    let kept_universal = combined.iter().filter(|r| r.is_universal).count();
    let sources: HashSet<&str> = combined.iter().map(|r| r.source_file.as_str()).collect();
    lore_debug!(
        "Codex {output_event_name}: injecting {} chunks ({} universal) from {} sources",
        combined.len(),
        kept_universal,
        sources.len(),
    );

    Ok(Some(HookOutput::additional_context(
        output_event_name,
        hook::format_imperative(&combined),
    )))
}

impl CodexHookInput {
    fn to_call_context(&self) -> CallContext {
        let transcript_tail = self.transcript_tail();
        let mut file_path = self.tool_input_str("file_path");
        let mut description = self.tool_input_str("description");

        if self.tool_name.as_deref() == Some("apply_patch")
            && let Some(patch) = self.tool_input_str("command")
        {
            file_path = first_apply_patch_file(&patch).or(file_path);
            description = Some(patch);
        }

        CallContext {
            tool_name: self.tool_name.clone(),
            command: self.tool_input_str("command"),
            file_path,
            description,
            prompt: None,
            transcript_tail,
        }
    }

    fn to_prompt_call_context(&self) -> CallContext {
        CallContext {
            prompt: self.prompt.clone(),
            ..CallContext::empty()
        }
    }

    fn tool_input_str(&self, key: &str) -> Option<String> {
        self.tool_input
            .as_ref()?
            .get(key)?
            .as_str()
            .map(String::from)
    }

    fn tool_response_text(&self) -> Option<String> {
        let response = self.tool_response.as_ref()?;
        response
            .as_str()
            .map(String::from)
            .or_else(|| {
                response
                    .get("stderr")
                    .and_then(serde_json::Value::as_str)
                    .map(String::from)
            })
            .or_else(|| {
                response
                    .get("result")
                    .and_then(|r| r.get("stderr"))
                    .and_then(serde_json::Value::as_str)
                    .map(String::from)
            })
    }

    fn transcript_tail(&self) -> Option<String> {
        self.transcript_path
            .as_deref()
            .and_then(|p| hook::validate_transcript_path(Path::new(p)))
            .and_then(|canonical| hook::last_user_message(&canonical))
    }

    fn session_dedup_path(&self) -> Option<PathBuf> {
        self.session_id.as_deref().map(hook::dedup_file_path)
    }
}

fn first_apply_patch_file(patch: &str) -> Option<String> {
    for line in patch.lines() {
        if let Some(path) = line.strip_prefix("*** Add File: ") {
            return Some(path.to_string());
        }
        if let Some(path) = line.strip_prefix("*** Update File: ") {
            return Some(path.to_string());
        }
        if let Some(path) = line.strip_prefix("*** Delete File: ") {
            return Some(path.to_string());
        }
    }
    None
}

fn looks_like_error_output(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    [
        "error",
        "failed",
        "failure",
        "panic",
        "exception",
        "traceback",
        "not found",
        "denied",
        "exit status",
        "abort",
    ]
    .iter()
    .any(|needle| lower.contains(needle))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_real_apply_patch_fixture_into_file_context() {
        let input: CodexHookInput = serde_json::from_str(include_str!(
            "../tests/fixtures/codex/pre_tool_use_apply_patch.json"
        ))
        .unwrap();
        let ctx = input.to_call_context();
        assert_eq!(ctx.tool_name.as_deref(), Some("apply_patch"));
        assert_eq!(
            ctx.file_path.as_deref(),
            Some("tmp/codex-apply-patch-fixture.txt")
        );
        assert!(
            ctx.description
                .as_deref()
                .unwrap()
                .contains("*** Begin Patch")
        );
    }

    #[test]
    fn prompt_fixture_populates_prompt_only() {
        let input: CodexHookInput = serde_json::from_str(include_str!(
            "../tests/fixtures/codex/user_prompt_submit.json"
        ))
        .unwrap();
        let ctx = input.to_prompt_call_context();
        assert!(
            ctx.prompt
                .as_deref()
                .unwrap()
                .contains("printf codex-bash-fixture")
        );
        assert!(ctx.command.is_none());
        assert!(ctx.tool_name.is_none());
    }

    #[test]
    fn bash_success_fixture_does_not_look_like_error() {
        let input: CodexHookInput = serde_json::from_str(include_str!(
            "../tests/fixtures/codex/post_tool_use_bash_success.json"
        ))
        .unwrap();
        assert!(!looks_like_error_output(
            &input.tool_response_text().unwrap()
        ));
    }

    #[test]
    fn bash_error_fixture_text_is_available() {
        let input: CodexHookInput = serde_json::from_str(include_str!(
            "../tests/fixtures/codex/post_tool_use_bash_error.json"
        ))
        .unwrap();
        assert_eq!(
            input.tool_response_text().as_deref(),
            Some("codex-fail-fixture\n")
        );
    }

    #[test]
    fn apply_patch_file_parser_handles_update() {
        let patch = "*** Begin Patch\n*** Update File: src/lib.rs\n@@\n-old\n+new\n*** End Patch\n";
        assert_eq!(first_apply_patch_file(patch).as_deref(), Some("src/lib.rs"));
    }
}
