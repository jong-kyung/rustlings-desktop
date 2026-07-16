use cargo_metadata::{diagnostic::DiagnosticLevel, Message};
use serde::{Deserialize, Serialize};
use std::{
    io::Cursor,
    path::{Path, PathBuf},
};

pub const MAX_DIAGNOSTICS: usize = 1_000;
pub const MAX_DIAGNOSTIC_FIELD_BYTES: usize = 64 * 1024;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ValidationStage {
    Build,
    Test,
    Clippy,
    Program,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    Error,
    Warning,
    Info,
    Hint,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct MonacoRange {
    pub start_line_number: usize,
    pub start_column: usize,
    pub end_line_number: usize,
    pub end_column: usize,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct NormalizedDiagnostic {
    pub stage: ValidationStage,
    pub severity: Severity,
    pub message: String,
    pub code: Option<String>,
    pub range: Option<MonacoRange>,
    pub source_digest: String,
}

#[derive(Debug, Default)]
pub(crate) struct ParsedCargoOutput {
    pub text: String,
    pub diagnostics: Vec<NormalizedDiagnostic>,
    pub executables: Vec<PathBuf>,
    pub saw_compiler_message: bool,
    pub build_finished: Option<bool>,
    pub build_script_executed: bool,
}

pub(crate) fn parse_cargo_output(
    stdout: &[u8],
    stage: ValidationStage,
    expected_relative_source: &str,
    expected_absolute_source: &Path,
    source: &[u8],
    source_digest: &str,
    expected_target: &str,
) -> ParsedCargoOutput {
    let mut parsed = ParsedCargoOutput::default();
    for record in stdout.split_inclusive(|byte| *byte == b'\n') {
        let Some(message) = Message::parse_stream(Cursor::new(record)).next() else {
            continue;
        };
        match message {
            Ok(Message::CompilerMessage(message)) => {
                parsed.saw_compiler_message = true;
                if let Some(rendered) = &message.message.rendered {
                    parsed.text.push_str(&bounded(rendered));
                    if !rendered.ends_with('\n') {
                        parsed.text.push('\n');
                    }
                }
                append_diagnostics(
                    &mut parsed.diagnostics,
                    &message.message,
                    stage,
                    expected_relative_source,
                    expected_absolute_source,
                    source,
                    source_digest,
                );
            }
            Ok(Message::CompilerArtifact(artifact)) => {
                if artifact.target.name == expected_target {
                    if let Some(executable) = artifact.executable {
                        parsed.executables.push(executable.into_std_path_buf());
                    }
                }
            }
            Ok(Message::BuildFinished(finished)) => {
                parsed.build_finished = Some(finished.success);
            }
            Ok(Message::BuildScriptExecuted(_)) => parsed.build_script_executed = true,
            Ok(Message::TextLine(_)) | Err(_) => {
                parsed.text.push_str(&String::from_utf8_lossy(record));
            }
            Ok(_) => parsed.text.push_str(&String::from_utf8_lossy(record)),
        }
    }
    parsed
}

fn append_diagnostics(
    output: &mut Vec<NormalizedDiagnostic>,
    diagnostic: &cargo_metadata::diagnostic::Diagnostic,
    stage: ValidationStage,
    expected_relative_source: &str,
    expected_absolute_source: &Path,
    source: &[u8],
    source_digest: &str,
) {
    if output.len() >= MAX_DIAGNOSTICS {
        return;
    }
    let severity = severity(diagnostic.level);
    let message = bounded(&diagnostic.message);
    let code = diagnostic.code.as_ref().map(|code| bounded(&code.code));
    let mut emitted = false;
    for span in diagnostic.spans.iter().filter(|span| {
        span.is_primary
            && is_expected_source(
                &span.file_name,
                expected_relative_source,
                expected_absolute_source,
            )
    }) {
        if output.len() >= MAX_DIAGNOSTICS {
            break;
        }
        output.push(NormalizedDiagnostic {
            stage,
            severity,
            message: message.clone(),
            code: code.clone(),
            range: normalize_span(source, span),
            source_digest: source_digest.to_owned(),
        });
        emitted = true;
    }
    if !emitted {
        output.push(NormalizedDiagnostic {
            stage,
            severity,
            message,
            code,
            range: None,
            source_digest: source_digest.to_owned(),
        });
    }
}

fn is_expected_source(file_name: &str, relative: &str, absolute: &Path) -> bool {
    file_name == relative || Path::new(file_name) == absolute
}

pub fn normalize_span(
    source: &[u8],
    span: &cargo_metadata::diagnostic::DiagnosticSpan,
) -> Option<MonacoRange> {
    let source = std::str::from_utf8(source).ok()?;
    let start = usize::try_from(span.byte_start).ok()?;
    let end = usize::try_from(span.byte_end).ok()?;
    if start > end
        || end > source.len()
        || !source.is_char_boundary(start)
        || !source.is_char_boundary(end)
        || span.line_start == 0
        || span.line_end == 0
        || span.column_start == 0
        || span.column_end == 0
    {
        return None;
    }
    let (start_line_number, start_column) = utf16_position(source, start);
    let (end_line_number, end_column) = utf16_position(source, end);
    if start_line_number != span.line_start || end_line_number != span.line_end {
        return None;
    }
    Some(MonacoRange {
        start_line_number,
        start_column,
        end_line_number,
        end_column,
    })
}

fn utf16_position(source: &str, offset: usize) -> (usize, usize) {
    let prefix = &source[..offset];
    let line_start = prefix.rfind('\n').map_or(0, |index| index + 1);
    (
        prefix.bytes().filter(|byte| *byte == b'\n').count() + 1,
        source[line_start..offset].encode_utf16().count() + 1,
    )
}

fn severity(level: DiagnosticLevel) -> Severity {
    match level {
        DiagnosticLevel::Ice | DiagnosticLevel::Error | DiagnosticLevel::FailureNote => {
            Severity::Error
        }
        DiagnosticLevel::Warning => Severity::Warning,
        DiagnosticLevel::Note => Severity::Info,
        DiagnosticLevel::Help => Severity::Hint,
        _ => Severity::Info,
    }
}

fn bounded(value: &str) -> String {
    if value.len() <= MAX_DIAGNOSTIC_FIELD_BYTES {
        return value.to_owned();
    }
    let mut end = MAX_DIAGNOSTIC_FIELD_BYTES;
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    value[..end].to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use cargo_metadata::diagnostic::DiagnosticSpan;

    fn span(start: u32, end: u32, start_line: usize, end_line: usize) -> DiagnosticSpan {
        serde_json::from_value(serde_json::json!({
            "file_name": "exercises/00_intro/intro1.rs",
            "byte_start": start,
            "byte_end": end,
            "line_start": start_line,
            "line_end": end_line,
            "column_start": 1,
            "column_end": 1,
            "is_primary": true,
            "text": [],
            "label": null,
            "suggested_replacement": null,
            "suggestion_applicability": null,
            "expansion": null
        }))
        .unwrap()
    }

    #[test]
    fn converts_ascii_emoji_multiline_and_zero_length_spans_to_utf16() {
        let source = "a😀b\nsecond";
        assert_eq!(
            normalize_span(source.as_bytes(), &span(1, 5, 1, 1)),
            Some(MonacoRange {
                start_line_number: 1,
                start_column: 2,
                end_line_number: 1,
                end_column: 4,
            })
        );
        assert_eq!(
            normalize_span(source.as_bytes(), &span(5, 9, 1, 2)),
            Some(MonacoRange {
                start_line_number: 1,
                start_column: 4,
                end_line_number: 2,
                end_column: 3,
            })
        );
        assert_eq!(
            normalize_span(source.as_bytes(), &span(9, 9, 2, 2)),
            Some(MonacoRange {
                start_line_number: 2,
                start_column: 3,
                end_line_number: 2,
                end_column: 3,
            })
        );
    }

    #[test]
    fn rejects_non_boundary_out_of_bounds_and_wrong_line_spans() {
        let source = "a😀b\n";
        assert!(normalize_span(source.as_bytes(), &span(2, 5, 1, 1)).is_none());
        assert!(normalize_span(source.as_bytes(), &span(0, 99, 1, 1)).is_none());
        assert!(normalize_span(source.as_bytes(), &span(0, 1, 2, 2)).is_none());
    }

    #[test]
    fn cargo_parser_preserves_unknown_and_text_records() {
        let stdout = b"startup text\n{\"reason\":\"future-message\",\"value\":1}\n";
        let parsed = parse_cargo_output(
            stdout,
            ValidationStage::Build,
            "exercises/00_intro/intro1.rs",
            Path::new("/snapshot/exercises/00_intro/intro1.rs"),
            b"fn main() {}\n",
            "digest",
            "intro1",
        );
        assert_eq!(parsed.text.as_bytes(), stdout);
        assert!(parsed.diagnostics.is_empty());
    }
}
