// Copyright Kani Contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! This module includes the implementation of the `report` subcommand.

use std::{
    fs::File,
    io::{BufRead, BufReader, IsTerminal},
    path::PathBuf,
};

use anyhow::Result;

use crate::args::ReportFormat;
use crate::coverage::{
    ColumnNumber, CovResult, LineNumber, LineResults, MarkerInfo, function_coverage_results,
    function_info_from_file, load_coverage_inputs,
};
use crate::summary::line_coverage_results;
use crate::args::ReportArgs;

/// Executes the `report` subcommand.
///
/// First, it loads the coverage metadata and results from the files passed as
/// arguments. Then, for each file referenced in the metadata, it computes its
/// associated coverage information on a per-function basis, producing a
/// human-readable report for each one of the files.
pub fn report_main(args: &ReportArgs) -> Result<()> {
    let (results, source_files) = load_coverage_inputs(&args.mapfile, &args.profile)?;

    let checked_format = check_format(&args.format);

    for file in source_files {
        let fun_info = function_info_from_file(&file);
        let mut file_cov_info = Vec::new();
        for info in fun_info {
            let cov_results = function_coverage_results(&info, &file, &results);
            let line_coverage = line_coverage_results(&info, &cov_results);
            let line_coverage_matched: LineResults =
                (info.start.0..=info.end.0).zip(line_coverage.clone()).collect();
            file_cov_info.push(line_coverage_matched);
        }
        output_coverage_results(&checked_format, file, file_cov_info)?;
    }

    Ok(())
}


/// Checks the `format` argument for the `report` subcommand and selects to
/// another format if the one we are checking is not possible for any reason.
///
/// For example, if the `Terminal` format is specified but we are not writing to
/// a terminal, it will auto-select the `Escapes` format to avoid character
/// issues.
fn check_format(format: &ReportFormat) -> ReportFormat {
    match format {
        ReportFormat::Terminal if std::io::stdout().is_terminal() => ReportFormat::Terminal,
        ReportFormat::Terminal | ReportFormat::Escapes => ReportFormat::Escapes,
    }
}

/// Output coverage results, and highlight regions or lines which are not
/// covered.
///
/// When highlighting the terminal, highlighted areas must be closed on the each
/// line to avoid highlighting beyond the source code area. Therefore, this
/// highlighting algorithm emits codes for both opening and closing the
/// highlighted area if any highlighting must be done for a line.
///
/// Therefore, a coverage region spanning multiple lines will be represented
/// with multiple highlighted lines. For example, let's take this uncovered
/// function:
///
/// fn _other_function() {
///     println!("Hello, world!");
/// }
///
/// It will be highlighted as follows:
///
/// ```fn _other_function() {'''
/// ```    println!("Hello, world!");'''
/// ```}'''
///
/// Note how there is always a corresponding escape for each line.
/// This is more sophisticated than the naive solution, which would simply emit two escapes
/// per region (one opening, one closing) as follows:
///
/// ```fn _other_function() {
///     println!("Hello, world!");
/// }'''
///
pub fn output_coverage_results(
    format: &ReportFormat,
    filepath: PathBuf,
    results: Vec<LineResults>,
) -> Result<()> {
    let flattened_results: LineResults = results.into_iter().flatten().collect();
    println!("{}", filepath.to_string_lossy());

    let file = File::open(filepath)?;
    let reader = BufReader::new(file);

    let mut must_highlight = false;

    for (i, line) in reader.lines().enumerate() {
        let idx = i + 1;
        let line = line?;

        let cur_line_result = flattened_results.iter().find(|(num, _)| *num == idx);

        let (max_times, line_fmt) = if let Some((_, Some((max, marker_info)))) = cur_line_result {
            match marker_info {
                MarkerInfo::FullLine => {
                    (Some(max), insert_escapes(&line, vec![(0, true), (line.len(), false)], format))
                }
                MarkerInfo::Markers(results) => {
                    let opening = results.iter().any(|m| {
                        m.times_covered == 0 && m.region.start.0 == idx && m.region.end.0 != idx
                    });
                    let closing = results.iter().any(|m| {
                        m.times_covered == 0 && m.region.start.0 != idx && m.region.end.0 == idx
                    });
                    let mut escapes = results
                        .iter()
                        .filter(|m| m.times_covered == 0)
                        .flat_map(|m| match (m.region.start.0 == idx, m.region.end.0 == idx) {
                            (true, true) => {
                                vec![(m.region.start.1 - 1, true), (m.region.end.1 - 1, false)]
                            }
                            (true, false) => vec![(m.region.start.1 - 1, true)],
                            (false, true) => vec![(m.region.end.1 - 1, false)],
                            (false, false) => vec![],
                        })
                        .collect::<Vec<(ColumnNumber, bool)>>();

                    if must_highlight && closing {
                        escapes.push((0, true));
                        must_highlight = false;
                    }
                    if opening {
                        escapes.push((line.len(), false));
                        must_highlight = true;
                    }
                    if must_highlight && escapes.is_empty() {
                        escapes.extend([(0, true), (line.len(), false)]);
                    }

                    // If any result points to an out-of-bounds region after the
                    // end of the line (due to a known issue), we extend the
                    // line with a space to avoid crashing when drawing it.
                    let line = if results_with_nonexisting_regions_in_line(results, &line, idx) {
                        format!("{line} ")
                    } else {
                        line
                    };

                    (Some(max), insert_escapes(&line, closing_escapes, format))
                }
            }
        } else {
            (
                None,
                if !must_highlight {
                    line
                } else {
                    insert_escapes(&line, vec![(0, true), (line.len(), false)], format)
                },
            )
        };

        let max_fmt =
            if let Some(num) = max_times { format!("{num:4}") } else { format!("{:4}", " ") };

        println!("{idx:4}| {max_fmt}| {line_fmt}");
    }

    Ok(())
}

/// Returns whether any of the coverage results for a line point to out-of-bound
/// regions.
///
/// This is helpful to detect cases in which, for an unknown reason, the Rust
/// coverage instrumentation inserts a single-column code span after the end of
/// a line. More details in <https://github.com/model-checking/kani/issues/3543>
fn results_with_nonexisting_regions_in_line(
    results: &[CovResult],
    line: &str,
    idx: LineNumber,
) -> bool {
    results.iter().any(|m| {
        m.region.start.0 == idx
            && m.region.end.0 == idx
            && m.region.end.1 - m.region.start.1 == 1
            && m.region.end.1 > line.len()
    })
}

/// Inserts opening/closing escape strings into `str` given a set of `markers`.
/// Each marker is a tuple `(offset, type)` where:
///  * `offset` represents the offset in which the marker must be inserted, and
///  * `type` represents whether it is an opening (`true`) or closing (`false`)
///    escape.
///
/// The specific escape to be used are determined by the report format.
fn insert_escapes(
    str: &str,
    mut markers: Vec<(ColumnNumber, bool)>,
    format: &ReportFormat,
) -> String {
    let (open, close) = match format {
        ReportFormat::Terminal => ("\x1b[41m", "\x1b[0m"),
        ReportFormat::Escapes => ("```", "'''"),
    };
    markers.sort();

    let mut escaped = str.to_owned();
    let mut offset = 0;
    for (i, opening) in markers {
        let escape = if opening { open } else { close };
        escaped.insert_str(i + offset, escape);
        offset += escape.len();
    }
    escaped
}
