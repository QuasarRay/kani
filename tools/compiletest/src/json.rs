// SPDX-License-Identifier: Apache-2.0 OR MIT
//
// Modifications Copyright Kani Contributors
// See GitHub history for details.

//! These structs are a subset of the ones found in `rustc_errors::json`.
//! They are only used for deserialization of JSON output provided by libtest.

use serde::Deserialize;
use std::path::PathBuf;

#[derive(Deserialize)]
struct Diagnostic {
    _message: String,
    _code: Option<DiagnosticCode>,
    _level: String,
    _children: Vec<Diagnostic>,
    rendered: Option<String>,
}

#[derive(Deserialize)]
struct ArtifactNotification {
    #[allow(dead_code)]
    artifact: PathBuf,
}

#[derive(Deserialize)]
struct FutureIncompatReport {
    future_incompat_report: Vec<FutureBreakageItem>,
}

#[derive(Deserialize)]
struct FutureBreakageItem {
    diagnostic: Diagnostic,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum CompilerOutput {
    Diagnostic(Diagnostic),
    FutureIncompat(FutureIncompatReport),
    Artifact(ArtifactNotification),
}

#[derive(Deserialize, Clone)]
#[allow(dead_code)]
struct DiagnosticSpanMacroExpansion {
    /// name of macro that was applied (e.g., "foo!" or "#[derive(Eq)]")
    _macro_decl_name: String,
}

#[derive(Deserialize, Clone)]
struct DiagnosticCode {
    /// The code itself.
    _code: String,
}

pub fn extract_rendered(output: &str) -> String {
    output
        .lines()
        .filter_map(|line| {
            if !line.starts_with('{') {
                return Some(format!("{line}\n"));
            }
            match serde_json::from_str::<CompilerOutput>(line) {
                Ok(CompilerOutput::Diagnostic(diagnostic)) => diagnostic.rendered,
                Ok(CompilerOutput::FutureIncompat(report)) if report.future_incompat_report.is_empty() => None,
                Ok(CompilerOutput::FutureIncompat(report)) => Some(format!(
                    "Future incompatibility report: {}",
                    report.future_incompat_report.into_iter().fold(String::new(), |mut output, item| {
                        use std::fmt::Write;
                        let _ = writeln!(output, "Future breakage diagnostic:");
                        let rendered =
                            item.diagnostic.rendered.unwrap_or_else(|| "Not rendered".to_string());
                        let _ = write!(output, "{rendered}");
                        output
                    })
                )),
                Ok(CompilerOutput::Artifact(_)) => None,
                Err(_) => {
                    print!("failed to decode compiler output as json: line: {line}\noutput: {output}");
                    panic!()
                }
            }
        })
        .collect()
}
