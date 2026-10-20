// Copyright Kani Contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! This module includes the implementation of the `summary` subcommand.

use std::path::Path;

use anyhow::Result;

use crate::{
    args::{SummaryArgs, SummaryFormat},
    coverage::{
        CovResult, CoverageMetric, CoverageRegion, FileCoverageInfo, Function, FunctionInfo,
        LineNumber, MarkerInfo, function_coverage_results, function_info_from_file,
        load_coverage_inputs,
    },
};

/// Executes the `summary` subcommand.
///
/// First, it loads the coverage metadata and results from the files passed as
/// arguments. Then, for each file referenced in the metadata, it computes its
/// associated coverage information on a per-function basis, producing coverage
/// metrics for each one of the files.
pub fn summary_main(args: &SummaryArgs) -> Result<()> {
    let (results, source_files) = load_coverage_inputs(&args.mapfile, &args.profile)?;

    let mut all_cov_info: Vec<FileCoverageInfo> = Vec::new();

    for file in source_files {
        let fun_info = function_info_from_file(&file);
        let mut file_cov_info = Vec::new();
        for info in fun_info {
            let cov_results = function_coverage_results(&info, &file, &results);
            let function_coverage = function_coverage_info(&cov_results);
            let line_coverage = line_coverage_info(&info, &cov_results);
            let region_coverage = region_coverage_info(&cov_results);
            let cur_function_coverage_results = FunctionCoverageResults {
                is_covered: function_coverage,
                total_lines: line_coverage.1,
                covered_lines: line_coverage.0,
                covered_regions: region_coverage.0,
                total_regions: region_coverage.1,
            };
            file_cov_info.push(cur_function_coverage_results);
        }
        let aggr_cov_info = calculate_cov_info(&file, &file_cov_info);
        all_cov_info.push(aggr_cov_info);
    }
    print_coverage_info(&all_cov_info, &args.format);

    Ok(())
}

/// Calculate the coverage information (metrics) for a file
fn calculate_cov_info(file: &Path, file_cov_info: &[FunctionCoverageResults]) -> FileCoverageInfo {
    let total_functions = file_cov_info.len();
    let covered_functions = file_cov_info.iter().filter(|f| f.is_covered).count();
    let fun_cov_info = CoverageMetric::new(covered_functions, total_functions);

    let covered_lines = file_cov_info.iter().map(|c| c.covered_lines).sum();
    let total_lines = file_cov_info.iter().map(|c| c.total_lines).sum();
    let lines_cov_info = CoverageMetric::new(covered_lines, total_lines);

    let covered_regions = file_cov_info.iter().map(|c| c.covered_regions).sum();
    let total_regions = file_cov_info.iter().map(|c| c.total_regions).sum();
    let region_cov_info = CoverageMetric::new(covered_regions, total_regions);

    FileCoverageInfo {
        filename: file.to_string_lossy().to_string(),
        function: fun_cov_info,
        line: lines_cov_info,
        region: region_cov_info,
    }
}

fn function_coverage_info(cov_results: &Option<(Function, Vec<CovResult>)>) -> bool {
    cov_results.as_ref().is_some_and(|res| res.1.iter().any(|c| c.times_covered > 0))
}

/// Function coverage results
struct FunctionCoverageResults {
    is_covered: bool,
    covered_lines: usize,
    total_lines: usize,
    covered_regions: usize,
    total_regions: usize,
}


/// Computes coverage results from a line-based perspective.
///
/// Basically, for each line we produce an `<Option<(usize, MarkerInfo)>>`
/// result where:
///  * `None` means there were no coverage results associated with this line.
///    This may happen in lines that only contain a closing `}`, for example.
///  * `Some(max, markers)` means there were coverage results associated with
///    the line or we deduced no results were possible based on function
///    information (i.e., the function was not reachable during verification).
///    Here, `max` represents the maximum number of times the line was covered by
///    any coverage result, and `markers` represents marker information which is
///    relevant to the line (including coverage results).
///
/// As a result, we essentially precompute here most of the information required
/// for the generation of coverage reports.
pub fn line_coverage_results(
    info: &FunctionInfo,
    fun_results: &Option<(Function, Vec<CovResult>)>,
) -> Vec<Option<(usize, MarkerInfo)>> {
    let start_line = info.start.0;
    let end_line = info.end.0;

    let mut line_status: Vec<Option<(usize, MarkerInfo)>> =
        Vec::with_capacity(end_line - start_line + 1);

    if let Some(res) = fun_results {
        let mut cur_results = res.1.clone();
        // Sort the results by row (descending).
        cur_results.sort_by_key(|a| std::cmp::Reverse(a.region.start.0));

        /// Checks if a line is relevant to a region.
        /// Here, we define "relevant" as the line appearing after/at the start
        /// of a region and before/at the end of a region.
        fn line_relevant_to_region(line: LineNumber, region: &CoverageRegion) -> bool {
            region.start.0 <= line && region.end.0 >= line
        }

        for line in start_line..end_line {
            // Filter results which are relevant to the current line
            let line_results: Vec<crate::coverage::CovResult> = cur_results
                .iter()
                .filter(|c| line_relevant_to_region(line, &c.region))
                .cloned()
                .collect();

            if line_results.is_empty() {
                line_status.push(None);
            } else {
                let max_covered = line_results
                    .iter()
                    .max_by_key(|res| res.times_covered)
                    .map(|res| res.times_covered)
                    .unwrap_or(0);
                line_status.push(Some((max_covered, MarkerInfo::Markers(line_results))));
            }
        }
    } else {
        line_status =
            std::iter::repeat_n(Some((0, MarkerInfo::FullLine)), end_line - start_line + 1)
                .collect();
    }
    line_status
}

/// Compute the number of covered lines and number of total lines given the
/// coverage results for a given function.
pub fn line_coverage_info(
    info: &FunctionInfo,
    fun_results: &Option<(Function, Vec<CovResult>)>,
) -> (usize, usize) {
    let line_status = line_coverage_results(info, fun_results);
    let total_lines = line_status.iter().filter(|s| s.is_some()).count();
    let covered_lines =
        line_status.iter().filter(|s| s.as_ref().is_some_and(|(count, _)| *count > 0)).count();
    (covered_lines, total_lines)
}

/// Compute the number of covered regions and number of total regions given the
/// coverage results for a given function.
fn region_coverage_info(fun_results: &Option<(Function, Vec<CovResult>)>) -> (usize, usize) {
    fun_results.as_ref().map_or((0, 0), |res| {
        (res.1.iter().filter(|c| c.times_covered > 0).count(), res.1.len())
    })
}

/// Output coverage information for a set of files
fn print_coverage_info(info: &[FileCoverageInfo], format: &SummaryFormat) {
    match format {
        SummaryFormat::Markdown => print_coverage_markdown_info(info),
    }
}

/// Output coverage information for a set of files in the markdown format
fn print_coverage_markdown_info(info: &[FileCoverageInfo]) {
    const HEADERS: [&str; 4] = ["Filename", "Function (%)", "Line (%)", "Region (%)"];

    fn metric(metric: &CoverageMetric) -> String {
        let rate = if metric.total == 0 {
            "N/A".to_string()
        } else {
            format!("{:.2}", metric.covered as f32 / metric.total as f32 * 100_f32)
        };
        format!("{}/{} ({rate})", metric.covered, metric.total)
    }

    let rows = info
        .iter()
        .map(|info| {
            [
                info.filename.clone(),
                metric(&info.function),
                metric(&info.line),
                metric(&info.region),
            ]
        })
        .collect::<Vec<_>>();
    let widths = std::array::from_fn(|i| {
        rows.iter().map(|row| row[i].len()).max().unwrap_or(0).max(HEADERS[i].len())
    });
    let row = |cells: [&str; 4]| {
        format!(
            "| {} |",
            cells
                .into_iter()
                .zip(widths)
                .map(|(cell, width)| format!("{cell:<width$}"))
                .collect::<Vec<_>>()
                .join(" | ")
        )
    };

    let header = row(HEADERS);
    let separator = format!("| {} |", widths.map(|width| "-".repeat(width)).join(" | "));
    let data = rows.iter().map(|cells| row(cells.each_ref().map(String::as_str)));
    println!("{}", [header, separator].into_iter().chain(data).collect::<Vec<_>>().join("\n"));
}
