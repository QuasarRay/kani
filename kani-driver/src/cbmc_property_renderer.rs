// Copyright Kani Contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

use crate::args::OutputFormat;
use crate::call_cbmc::{FailedProperties, VerificationStatus};
use crate::cbmc_output_parser::{CheckStatus, ParserItem, Property, TraceItem};
use crate::coverage::cov_results::CoverageResults;
use console::style;
use once_cell::sync::Lazy;
use regex::Regex;
use rustc_demangle::demangle;
use std::collections::HashMap;
use std::path::PathBuf;

type CbmcAltDescriptions = HashMap<&'static str, Vec<(&'static str, Option<&'static str>)>>;

/// Hash map that relates property classes with descriptions, used by
/// `get_readable_description` to provide user friendly descriptions.
/// See the comment in `get_readable_description` for more information on
/// how this data structure is used.
static CBMC_ALT_DESCRIPTIONS: Lazy<CbmcAltDescriptions> = Lazy::new(|| {
    let mut map = HashMap::new();
    map.insert("error_label", vec![]);
    map.insert("division-by-zero", vec![("division by zero", None)]);
    map.insert("enum-range-check", vec![("enum range check", None)]);
    map.insert(
        "undefined-shift",
        vec![
            ("shift distance is negative", None),
            ("shift distance too large", None),
            ("shift operand is negative", None),
            ("shift of non-integer type", None),
        ],
    );
    map.insert(
        "overflow",
        vec![
            ("result of signed mod is not representable", None),
            ("arithmetic overflow on signed type conversion", None),
            ("arithmetic overflow on signed division", None),
            ("arithmetic overflow on signed unary minus", None),
            ("arithmetic overflow on signed shl", None),
            ("arithmetic overflow on unsigned unary minus", None),
            ("arithmetic overflow on signed +", Some("arithmetic overflow on signed addition")),
            ("arithmetic overflow on signed -", Some("arithmetic overflow on signed subtraction")),
            (
                "arithmetic overflow on signed *",
                Some("arithmetic overflow on signed multiplication"),
            ),
            ("arithmetic overflow on unsigned +", Some("arithmetic overflow on unsigned addition")),
            (
                "arithmetic overflow on unsigned -",
                Some("arithmetic overflow on unsigned subtraction"),
            ),
            (
                "arithmetic overflow on unsigned *",
                Some("arithmetic overflow on unsigned multiplication"),
            ),
            ("arithmetic overflow on floating-point typecast", None),
            ("arithmetic overflow on floating-point division", None),
            ("arithmetic overflow on floating-point addition", None),
            ("arithmetic overflow on floating-point subtraction", None),
            ("arithmetic overflow on floating-point multiplication", None),
            ("arithmetic overflow on unsigned to signed type conversion", None),
            ("arithmetic overflow on float to signed integer type conversion", None),
            ("arithmetic overflow on signed to unsigned type conversion", None),
            ("arithmetic overflow on unsigned to unsigned type conversion", None),
            ("arithmetic overflow on float to unsigned integer type conversion", None),
        ],
    );
    map.insert(
        "NaN",
        vec![
            ("NaN on +", Some("NaN on addition")),
            ("NaN on -", Some("NaN on subtraction")),
            ("NaN on /", Some("NaN on division")),
            ("NaN on *", Some("NaN on multiplication")),
        ],
    );
    map.insert(
        "pointer_arithmetic",
        vec![
            ("pointer relation: deallocated dynamic object", None),
            ("pointer relation: dead object", None),
            ("pointer relation: pointer NULL", None),
            ("pointer relation: pointer invalid", None),
            ("pointer relation: pointer outside dynamic object bounds", None),
            ("pointer relation: pointer outside object bounds", None),
            ("pointer relation: invalid integer address", None),
            ("pointer arithmetic: deallocated dynamic object", None),
            ("pointer arithmetic: dead object", None),
            ("pointer arithmetic: pointer NULL", None),
            ("pointer arithmetic: pointer invalid", None),
            ("pointer arithmetic: pointer outside dynamic object bounds", None),
            ("pointer arithmetic: pointer outside object bounds", None),
            ("pointer arithmetic: invalid integer address", None),
        ],
    );
    map.insert(
        "pointer_dereference",
        vec![
            (
                "dereferenced function pointer must be",
                Some("dereference failure: invalid function pointer"),
            ),
            ("dereference failure: pointer NULL", None),
            ("dereference failure: pointer invalid", None),
            ("dereference failure: deallocated dynamic object", None),
            ("dereference failure: dead object", None),
            ("dereference failure: pointer outside dynamic object bounds", None),
            ("dereference failure: pointer outside object bounds", None),
            ("dereference failure: invalid integer address", None),
        ],
    );
    // These are very hard to understand without more context.
    map.insert(
        "pointer_primitives",
        vec![
            ("pointer invalid", None),
            ("deallocated dynamic object", Some("pointer to deallocated dynamic object")),
            ("dead object", Some("pointer to dead object")),
            ("pointer outside dynamic object bounds", None),
            ("pointer outside object bounds", None),
            ("invalid integer address", None),
        ],
    );
    map.insert(
        "array_bounds",
        vec![
            ("lower bound", Some("index out of bounds")),
            // This one is redundant:
            // ("dynamic object upper bound", Some("access out of bounds")),
            (
                "upper bound",
                Some("index out of bounds: the length is less than or equal to the given index"),
            ),
        ],
    );
    map.insert(
        "bit_count",
        vec![
            ("count trailing zeros is undefined for value zero", None),
            ("count leading zeros is undefined for value zero", None),
        ],
    );
    map.insert("memory-leak", vec![("dynamically allocated memory never freed", None)]);
    // These pre-conditions should not print temporary variables since they are embedded in the libc implementation.
    // They are added via `__CPROVER_precondition`.
    // map.insert("precondition_instance": vec![]);
    map
});

const UNSUPPORTED_CONSTRUCT_DESC: &str = "is not currently supported by Kani";
const UNWINDING_ASSERT_DESC: &str = "unwinding assertion loop";
const UNWINDING_ASSERT_REC_DESC: &str = "recursion unwinding assertion";
const UNDEFINED_FUNCTION_DESC: &str = "undefined function should be unreachable";

impl ParserItem {
    /// Determines if an item must be skipped or not.
    fn must_be_skipped(&self) -> bool {
        matches!(&self, ParserItem::Message { message_text, .. } if message_text.starts_with("Building error trace") || message_text.starts_with("VERIFICATION"))
    }
}

/// This is called "live" as CBMC output is streamed in, and we
/// filter and transform it into the format we expect.
///
/// This will output "messages" live as they stream in if `output_format` is
/// set to `regular` but will otherwise not print. When a log file path is provided, output is
/// redirected to the log file instead.
pub fn kani_cbmc_output_filter(
    item: ParserItem,
    extra_ptr_checks: bool,
    quiet: bool,
    output_format: &OutputFormat,
    log_file: Option<&PathBuf>,
) -> Option<ParserItem> {
    // Some items (e.g., messages) are skipped.
    // We could also process them and decide to skip later.
    if item.must_be_skipped() {
        return None;
    }
    let processed_item = process_item(item, extra_ptr_checks);
    // Both formatting and printing could be handled by objects which
    // implement a trait `Printer`.
    if !quiet {
        let formatted_item = format_item(&processed_item, output_format);
        if let Some(fmt_item) = formatted_item {
            if let Some(log_path) = log_file {
                if let Err(e) = crate::log_file::append_line(log_path, &fmt_item) {
                    eprintln!(
                        "Failed to write CBMC output to log file {}: {}",
                        log_path.display(),
                        e
                    );
                }
            } else {
                println!("{fmt_item}");
            }
        }
    }
    // TODO: Record processed items and dump them into a JSON file
    // <https://github.com/model-checking/kani/issues/942>
    Some(processed_item)
}

/// Processes a `ParserItem`. In general, all items are returned as they are,
/// except for:
///  * Error messages, which may be edited.
///  * Verification results, which must be postprocessed.
fn process_item(item: ParserItem, extra_ptr_checks: bool) -> ParserItem {
    match item {
        ParserItem::Result { result } => {
            let postprocessed_result = postprocess_result(result, extra_ptr_checks);
            ParserItem::Result { result: postprocessed_result }
        }
        ParserItem::Message { ref message_type, .. } if message_type == "ERROR" => {
            postprocess_error_message(item)
        }
        item => item,
    }
}

/// Returns an optional formatted item based on the output format
fn format_item(item: &ParserItem, output_format: &OutputFormat) -> Option<String> {
    match output_format {
        OutputFormat::Old => todo!(),
        OutputFormat::Regular => format_item_regular(item),
        OutputFormat::Terse => format_item_terse(item),
    }
}

/// Formats an item using the regular output format
fn format_item_regular(item: &ParserItem) -> Option<String> {
    match item {
        ParserItem::Program { program } => Some(program.to_string()),
        ParserItem::Message { message_text, .. } => Some(message_text.to_string()),
        _ => None,
    }
}

/// Formats an item using the terse output format
fn format_item_terse(_item: &ParserItem) -> Option<String> {
    None
}

/// Formats a result item (i.e., the complete set of verification checks).
/// This could be split into two functions for clarity, but at the moment
/// it uses the flag `show_checks` which depends on the output format.
///
/// This function reports the results of normal checks (e.g. assertions and
/// arithmetic overflow checks) and cover properties (specified using the
/// `kani::cover` macro) separately. Cover properties currently do not impact
/// the overall verification success or failure.
///
/// TODO: We could `write!` to `result_str` instead
/// <https://github.com/model-checking/kani/issues/1480>
pub fn format_result(
    properties: &Vec<Property>,
    status: VerificationStatus,
    should_panic: bool,
    failed_properties: FailedProperties,
    show_checks: bool,
) -> String {
    let mut result_str = String::new();
    let mut number_checks_failed = 0;
    let mut number_checks_unreachable = 0;
    let mut number_checks_undetermined = 0;
    let mut failed_tests: Vec<&Property> = vec![];

    // cover checks
    let mut number_covers_satisfied = 0;
    let mut number_covers_undetermined = 0;
    let mut number_covers_unreachable = 0;
    let mut number_covers_unsatisfiable = 0;

    if show_checks {
        result_str.push_str("\nRESULTS:\n");
    }

    for (index, prop) in (1..).zip(properties) {
        let name = prop.property_name();
        let status = &prop.status;
        let description = &prop.description;
        let location = &prop.source_location;

        match status {
            CheckStatus::Failure => {
                number_checks_failed += 1;
                failed_tests.push(prop);
            }
            CheckStatus::Undetermined => {
                if prop.is_cover_property() {
                    number_covers_undetermined += 1;
                } else {
                    number_checks_undetermined += 1;
                }
            }
            CheckStatus::Unreachable => {
                if prop.is_cover_property() {
                    number_covers_unreachable += 1;
                } else {
                    number_checks_unreachable += 1;
                }
            }
            CheckStatus::Satisfied => {
                assert!(prop.is_cover_property());
                number_covers_satisfied += 1;
            }
            CheckStatus::Unsatisfiable => {
                assert!(prop.is_cover_property());
                number_covers_unsatisfiable += 1;
            }
            _ => (),
        }

        if show_checks {
            let check_id = format!("Check {index}: {name}\n");
            let status_msg = format!("\t - Status: {status}\n");
            let description_msg = format!("\t - Description: \"{description}\"\n");

            result_str.push_str(&check_id);
            result_str.push_str(&status_msg);
            result_str.push_str(&description_msg);

            if !location.is_missing() {
                let location_msg = format!("\t - Location: {location}\n");
                result_str.push_str(&location_msg);
            }
            result_str.push('\n');
        }
    }

    if show_checks {
        result_str.push_str("\nSUMMARY:");
    } else {
        result_str.push_str("\nVERIFICATION RESULT:");
    }

    let number_cover_properties = number_covers_satisfied
        + number_covers_unreachable
        + number_covers_unsatisfiable
        + number_covers_undetermined;

    let number_properties = properties.len() - number_cover_properties;

    let summary = format!("\n ** {number_checks_failed} of {number_properties} failed");
    result_str.push_str(&summary);

    let mut other_status = Vec::<String>::new();
    if number_checks_undetermined > 0 {
        let undetermined_str = format!("{number_checks_undetermined} undetermined");
        other_status.push(undetermined_str);
    }
    if number_checks_unreachable > 0 {
        let unreachable_str = format!("{number_checks_unreachable} unreachable");
        other_status.push(unreachable_str);
    }
    if !other_status.is_empty() {
        result_str.push_str(" (");
        result_str.push_str(&other_status.join(","));
        result_str.push(')');
    }
    result_str.push('\n');

    if number_cover_properties > 0 {
        // Print a summary line for cover properties
        let summary = format!(
            "\n ** {number_covers_satisfied} of {number_cover_properties} cover properties satisfied"
        );
        result_str.push_str(&summary);
        let mut other_status = Vec::<String>::new();
        if number_covers_undetermined > 0 {
            let undetermined_str = format!("{number_covers_undetermined} undetermined");
            other_status.push(undetermined_str);
        }
        if number_covers_unreachable > 0 {
            let unreachable_str = format!("{number_covers_unreachable} unreachable");
            other_status.push(unreachable_str);
        }
        if !other_status.is_empty() {
            result_str.push_str(" (");
            result_str.push_str(&other_status.join(","));
            result_str.push(')');
        }
        result_str.push('\n');
        result_str.push('\n');
    }

    for prop in failed_tests {
        let failure_message = build_failure_message(prop.description.clone(), &prop.trace.clone());
        result_str.push_str(&failure_message);
    }

    let verification_result = if status == VerificationStatus::Success {
        style("SUCCESSFUL").green()
    } else {
        style("FAILED").red()
    };
    let should_panic_info = if should_panic {
        match failed_properties {
            FailedProperties::None => " (encountered no panics, but at least one was expected)",
            FailedProperties::PanicsOnly => " (encountered one or more panics as expected)",
            FailedProperties::Other => {
                " (encountered failures other than panics, which were unexpected)"
            }
            FailedProperties::Error => " (encountered a solver error)",
        }
    } else {
        ""
    };
    let overall_result = format!("\nVERIFICATION:- {verification_result}{should_panic_info}\n");
    result_str.push_str(&overall_result);

    // Ideally, we should generate two `ParserItem::Message` and push them
    // into the parser iterator so they are the next messages to be processed.
    // However, we haven't figured out the best way to do this for now.
    // <https://github.com/model-checking/kani/issues/1432>
    if has_check_failure(properties, UNSUPPORTED_CONSTRUCT_DESC) {
        result_str.push_str(
            "** WARNING: A Rust construct that is not currently supported \
        by Kani was found to be reachable. Check the results for \
        more details.\n",
        );
    }
    if has_unwinding_assertion_failures(properties) {
        result_str.push_str("[Kani] info: Verification output shows one or more unwinding failures.\n\
        [Kani] tip: Consider increasing the unwinding value or disabling `--unwinding-assertions`.\n");
    }

    result_str
}

/// Separate checks into coverage and non-coverage based on property class and
/// format them separately for `--coverage`. Then we report both verification
/// and processed coverage results.
///
/// Note: The reporting of coverage results should be removed once `kani-cov` is
/// introduced.
pub fn format_coverage(
    properties: &[Property],
    cov_results: &CoverageResults,
    status: VerificationStatus,
    should_panic: bool,
    failed_properties: FailedProperties,
    show_checks: bool,
) -> String {
    let (_coverage_checks, non_coverage_checks): (Vec<Property>, Vec<Property>) =
        properties.iter().cloned().partition(|x| x.property_class() == "code_coverage");

    let verification_output =
        format_result(&non_coverage_checks, status, should_panic, failed_properties, show_checks);
    let cov_results_intro = "Source-based code coverage results:";
    let result = format!("{verification_output}\n{cov_results_intro}\n\n{cov_results}");

    result
}

/// Attempts to build a message for a failed property with as much detailed
/// information on the source location as possible.
fn build_failure_message(description: String, trace: &Option<Vec<TraceItem>>) -> String {
    let backup_failure_message = format!("Failed Checks: {description}\n");
    if trace.is_none() {
        return backup_failure_message;
    }
    let failure_trace = trace.clone().unwrap();

    let failure_source_wrap = failure_trace[failure_trace.len() - 1].source_location.clone();
    if failure_source_wrap.is_none() {
        return backup_failure_message;
    }
    let failure_source = failure_source_wrap.unwrap();

    if let Some(failure_file) = failure_source.file
        && let Some(failure_function) = failure_source.function
        && let Some(failure_line) = failure_source.line
    {
        return format!(
            "Failed Checks: {description}\n File: \"{failure_file}\", line {failure_line}, in {failure_function}\n"
        );
    }
    backup_failure_message
}

/// Edits an error message.
///
/// At present, we only know one case where CBMC emits an error message, related
/// to `--object-bits` being too low. The message is edited to show Kani
/// options.
fn postprocess_error_message(message: ParserItem) -> ParserItem {
    if let ParserItem::Message { ref message_text, message_type: _ } = message
        && message_text.contains("use the `--object-bits n` option")
    {
        ParserItem::Message {
            message_text: message_text
                .replace("--object-bits ", "-Z unstable-options --cbmc-args --object-bits "),
            message_type: String::from("ERROR"),
        }
    } else {
        message
    }
}

/// Postprocess verification results to check for certain cases (e.g. a reachable unsupported construct or a failed
/// unwinding assertion), and update the results of impacted checks accordingly.
///
/// This postprocessing follows the same steps:
///     1. Change all "SUCCESS" results to "UNDETERMINED" if the reachability check
///     for a Rust construct that is not currently supported by Kani failed, since
///     the missing exploration of execution paths through the unsupported construct
///     may hide failures
///     2. Change a check's result from "SUCCESS" to "UNREACHABLE" if its
///     reachability check's result was "SUCCESS"
///     3. Change results from "SUCCESS" to "UNDETERMINED" if an unwinding
///     assertion failed, since the insufficient unwinding may cause some execution
///     paths to be left unexplored.
///
///     Additionally, print a message at the end of the output that indicates if any
///     of the special cases above was hit.
pub fn postprocess_result(properties: Vec<Property>, extra_ptr_checks: bool) -> Vec<Property> {
    // First, determine if there are reachable unsupported constructs or unwinding assertions
    let has_reachable_unsupported_constructs =
        has_check_failure(&properties, UNSUPPORTED_CONSTRUCT_DESC);
    let has_failed_unwinding_asserts = has_unwinding_assertion_failures(&properties);
    // Then, determine if there are reachable undefined functions, and change
    // their description to highlight this fact
    let (properties_with_undefined, has_reachable_undefined_functions) =
        modify_undefined_function_checks(properties);
    // Split all properties into two groups: Regular properties and reachability checks
    let (properties, reach_checks) = split_properties(properties_with_undefined);
    // Annotate properties with the results of reachability checks.
    let properties_annotated = annotate_properties_with_reach_results(properties, reach_checks);
    let has_fundamental_failures = has_reachable_unsupported_constructs
        || has_failed_unwinding_asserts
        || has_reachable_undefined_functions;
    finalize_properties(properties_annotated, extra_ptr_checks, has_fundamental_failures)
}

/// Determines if there is property with status `FAILURE` and the given description
fn has_check_failure(properties: &[Property], description: &str) -> bool {
    properties
        .iter()
        .any(|prop| prop.status == CheckStatus::Failure && prop.description.contains(description))
}

// Determines if there were unwinding assertion failures in a set of properties
fn has_unwinding_assertion_failures(properties: &[Property]) -> bool {
    has_check_failure(properties, UNWINDING_ASSERT_DESC)
        || has_check_failure(properties, UNWINDING_ASSERT_REC_DESC)
}

/// Replaces the description of all properties from functions with a missing
/// definition.
fn modify_undefined_function_checks(mut properties: Vec<Property>) -> (Vec<Property>, bool) {
    let mut has_unknown_location_checks = false;
    for prop in &mut properties {
        if let Some(function) = &prop.source_location.function
            && prop.description == UNDEFINED_FUNCTION_DESC
        {
            // Missing functions come with mangled names.
            // `demangle` produces the demangled version if it's a mangled name.
            prop.description =
                format!("Function `{:#}` with missing definition is unreachable", demangle(function));
            has_unknown_location_checks |= prop.status == CheckStatus::Failure;
        };
    }
    (properties, has_unknown_location_checks)
}

/// Returns a user friendly property description.
///
/// `CBMC_ALT_DESCRIPTIONS` is a hash map where:
///  * The key is a property class.
///  * The value is a vector of pairs. In each of these pairs, the first member
///    is a description used to match (with method `contains`) on the original
///    property. If a match is found, we inspect the second member:
///     * If it's `None`, we replace the original property with the description
///       used to match.
///     * If it's `Some(string)`, we replace the original property with `string`.
///
/// For CBMC checks, this will ensure that check failures do not include any
/// temporary variable in their descriptions.
fn get_readable_description(property: &Property) -> String {
    let original = property.description.clone();
    CBMC_ALT_DESCRIPTIONS
        .get(property.property_class().as_str())
        .and_then(|alternatives| {
            alternatives.iter().find(|(description, _)| original.contains(description))
        })
        .map(|(description, replacement)| replacement.unwrap_or(*description).to_string())
        .unwrap_or(original)
}

/// Apply the ordered final property transformations after reachability annotation.
fn finalize_properties(
    mut properties: Vec<Property>,
    extra_ptr_checks: bool,
    has_fundamental_failures: bool,
) -> Vec<Property> {
    let check_id = Regex::new(r"\[KANI_CHECK_ID_([^\]]*)\] ").unwrap();
    for prop in &mut properties {
        prop.description = check_id.replace(&prop.description, "").to_string();
    }
    if !extra_ptr_checks {
        properties.retain(|prop| {
            let class = prop.property_class();
            !class.contains("pointer_arithmetic") && !class.contains("pointer_primitives")
        });
    }

    for prop in &mut properties {
        prop.description = get_readable_description(prop);
        if has_fundamental_failures && prop.status == CheckStatus::Success {
            prop.status = CheckStatus::Undetermined;
        } else if !has_fundamental_failures && prop.reach == Some(CheckStatus::Success) {
            assert!(
                prop.status == CheckStatus::Success,
                "** ERROR: Expecting the unreachable property \"{}\" to have a status of \"SUCCESS\"",
                prop.description
            );
            prop.status = CheckStatus::Unreachable;
        }

        if prop.is_code_coverage_property() {
            prop.status = match prop.status {
                CheckStatus::Success => CheckStatus::Uncovered,
                CheckStatus::Failure => CheckStatus::Covered,
                _ => unreachable!(
                    "status for coverage checks should be either `SUCCESS` or `FAILURE` prior to postprocessing"
                ),
            };
        }
        if prop.is_cover_property() {
            prop.status = match prop.status {
                CheckStatus::Success => CheckStatus::Unsatisfiable,
                CheckStatus::Failure => CheckStatus::Satisfied,
                status => status,
            };
        }
    }
    properties
}

/// Split reachability checks from user-facing properties, dropping successful sanity checks.
fn split_properties(properties: Vec<Property>) -> (Vec<Property>, Vec<Property>) {
    let mut regular = vec![];
    let mut reach = vec![];
    for prop in properties {
        match prop.property_class().as_str() {
            "reachability_check" => reach.push(prop),
            "sanity_check" if prop.status == CheckStatus::Success => {}
            _ => regular.push(prop),
        }
    }
    (regular, reach)
}

/// Filters out properties related to extra pointer checks
///
/// Our support for primitives and overflow pointer checks is unstable and
/// can result in lots of spurious failures. By default, we filter them out.
fn filter_ptr_checks(properties: Vec<Property>) -> Vec<Property> {
    properties
        .into_iter()
        .filter(|prop| {
            !prop.property_class().contains("pointer_arithmetic")
                && !prop.property_class().contains("pointer_primitives")
        })
        .collect()
}

/// When assertion reachability checks are turned on, Kani prefixes each
/// assert's description with an ID of the following form:
/// ```text
/// [KANI_CHECK_ID_<crate-name>_<index-of-check>]
/// ```
/// e.g.:
/// ```text
/// [KANI_CHECK_ID_foo.6875c808::foo_0] assertion failed: x % 2 == 0
/// ```
/// In addition, the description of each reachability check that it generates
/// includes the ID of the assert for which we want to check its reachability.
/// The description of a reachability check uses the following template:
/// ```text
/// <ID of original assert>
/// ```
/// e.g.:
/// ```text
/// KANI_CHECK_ID_foo.6875c808::foo_0
/// ```
/// This function first collects all data from reachability checks. Then,
/// it updates the reachability status for all properties accordingly.
fn annotate_properties_with_reach_results(
    mut properties: Vec<Property>,
    reach_checks: Vec<Property>,
) -> Vec<Property> {
    let mut reach_map: HashMap<String, Vec<CheckStatus>> = HashMap::new();
    let reach_desc_pat = Regex::new("KANI_CHECK_ID_.*_([0-9])*").unwrap();
    // Collect data (ID, status) from reachability checks
    for reach_check in reach_checks {
        let description = reach_check.description;
        // Capture the ID in the reachability check
        let check_id =
            reach_desc_pat.captures(description.as_str()).unwrap().get(0).unwrap().as_str();
        let check_id_str = format!("[{check_id}]");
        // Get the status and insert into `reach_map`
        let status = reach_check.status;
        reach_map.entry(check_id_str).or_default().push(status);
    }

    let check_marker_pat = Regex::new(r"\[KANI_CHECK_ID_([^\]]*)\]").unwrap();
    for prop in &mut properties {
        let Some(id) =
            check_marker_pat.captures(&prop.description).and_then(|captures| captures.get(0))
        else {
            continue;
        };
        if let Some(statuses) = reach_map.get(id.as_str()) {
            for status in statuses {
                // Report if any copy of `prop` is not success.
                if prop.reach.is_none_or(|reach| {
                    matches!(reach, CheckStatus::Satisfied | CheckStatus::Success)
                }) {
                    prop.reach = Some(*status);
                }
            }
        }
    }
    properties
}
