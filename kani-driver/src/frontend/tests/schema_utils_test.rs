// Copyright Kani Contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

/// Tests for the schema_utils module
/// This module contains tests for the schema_utils module
/// and the json_handler module
use crate::call_cbmc::{ExitStatus, FailedProperties, VerificationResult, VerificationStatus};
use crate::cbmc_output_parser::{CheckStatus, Property, PropertyId, SourceLocation};
use crate::frontend::JsonHandler;
use crate::frontend::schema_utils::{
    add_runner_results_to_json, create_harness_metadata_json, create_metadata_json,
    create_project_metadata_json, create_verification_result_json,
    create_verification_summary_json,
};
use crate::harness_runner::HarnessResult;
use crate::project::Project;
use kani_metadata::{HarnessAttributes, HarnessKind, HarnessMetadata, KaniMetadata};
use std::path::PathBuf;
use std::time::Duration;

fn harness(name: &str, file: &str) -> HarnessMetadata {
    HarnessMetadata {
        pretty_name: name.into(),
        mangled_name: name.replace("::", "_"),
        crate_name: "sample".into(),
        original_file: file.into(),
        original_start_line: 1,
        original_end_line: 2,
        goto_file: None,
        attributes: HarnessAttributes::new(HarnessKind::Proof),
        contract: None,
        has_loop_contracts: false,
        is_automatically_generated: false,
        is_bounded: false,
        is_ctor_based: false,
    }
}

fn property(
    id: u32,
    function: &str,
    class: &str,
    status: CheckStatus,
    description: &str,
    file: &str,
    line: &str,
    column: &str,
) -> Property {
    Property {
        property_id: PropertyId { id, fn_name: Some(function.into()), class: class.into() },
        status,
        description: description.into(),
        source_location: SourceLocation {
            file: Some(file.into()),
            function: Some(function.into()),
            line: Some(line.into()),
            column: Some(column.into()),
        },
        reach: None,
        trace: None,
    }
}

fn failure_result(results: Result<Vec<Property>, ExitStatus>) -> VerificationResult {
    VerificationResult {
        status: VerificationStatus::Failure,
        failed_properties: FailedProperties::Other,
        results,
        runtime: Duration::from_millis(120),
        generated_concrete_test: false,
        ignored_quantifiers: 0,
        coverage_results: None,
        cbmc_stats: None,
    }
}
#[test]
fn test_create_metadata_json() {
    let json = create_metadata_json();

    assert!(json.is_object());
    assert_eq!(json["version"], "1.0");
    assert!(json["timestamp"].as_str().unwrap().contains('T'));
    assert!(["debug", "release"].contains(&json["build_mode"].as_str().unwrap()));
}

#[test]
fn test_create_project_metadata_json() {
    let mut project = Project::default();
    let metadata = KaniMetadata {
        crate_name: "sample_crate".to_string(),
        proof_harnesses: vec![],
        test_harnesses: vec![],
        unsupported_features: vec![],
        contracted_functions: vec![],
        autoharness_md: None,
    };
    project.outdir = PathBuf::from("/tmp/outdir");
    project.metadata.push(metadata);

    let json = create_project_metadata_json(&project);
    assert_eq!(json["crate_name"][0], "sample_crate");
    assert_eq!(json["output_dir"], "/tmp/outdir");
    // No Cargo metadata on a standalone project, so there is no workspace root to report.
    assert!(json["workspace_root"].is_null());
}

#[test]
fn test_create_harness_metadata_json() {
    let mut harness = harness("crate::mod::my_harness", "src/lib.rs");
    harness.mangled_name = "mangled::harness".into();
    harness.crate_name = "sample_crate".into();
    harness.original_start_line = 10;
    harness.original_end_line = 20;
    harness.goto_file = Some("target/goto_file.goto".into());
    harness.has_loop_contracts = true;
    // Check that JSON consumers can distinguish bounded/constructor-based autoharnesses.
    harness.is_automatically_generated = true;
    harness.is_bounded = true;
    harness.is_ctor_based = true;

    let json = create_harness_metadata_json(&harness);

    assert_eq!(json["pretty_name"], "crate::mod::my_harness");
    assert_eq!(json["crate_name"], "sample_crate");
    assert_eq!(json["source"]["file"], "src/lib.rs");
    assert_eq!(json["source"]["start_line"], 10);
    assert_eq!(json["source"]["end_line"], 20);
    assert_eq!(json["has_loop_contracts"], true);
    assert_eq!(json["is_automatically_generated"], true);
    assert_eq!(json["is_bounded"], true);
    assert_eq!(json["is_ctor_based"], true);
}

#[test]
fn test_create_verification_result_json() {
    let mut harness = harness("crate::my_harness", "src/lib.rs");
    harness.mangled_name = "mangled_name".into();
    harness.crate_name = "sample_crate".into();

    let properties = vec![
        property(1, "foo", "safety", CheckStatus::Success, "no overflow", "src/lib.rs", "42", "5"),
        property(
            2,
            "bar",
            "assertion",
            CheckStatus::Failure,
            "assert failed",
            "src/main.rs",
            "10",
            "3",
        ),
    ];

    let verification_result = failure_result(Ok(properties));

    let harness_result = HarnessResult { harness: &harness, result: verification_result };

    let json = create_verification_result_json(&harness_result);

    // --- Assertions ---
    assert_eq!(json["harness_id"], "crate::my_harness");
    assert_eq!(json["status"], "Failure");
    assert_eq!(json["checks"][0]["function"], "foo");
    assert_eq!(json["checks"][1]["function"], "bar");
    assert_eq!(json["checks"][1]["location"]["file"], "src/main.rs");

    // Optional extra check
    assert!(json["duration_ms"].as_u64().unwrap() >= 100);
}

#[test]
fn test_create_verification_summary_json_real() {
    let mut harness = harness("foo::harness_ok", "src/lib.rs");
    harness.original_start_line = 10;
    harness.original_end_line = 20;
    let verification_result = VerificationResult::mock_success();

    let harness_result = HarnessResult { harness: &harness, result: verification_result };

    let json = create_verification_summary_json(&[harness_result], 1, "Completed");

    assert_eq!(json["summary"]["status"], "Completed");
    assert_eq!(json["summary"]["total_harnesses"], 1);
    assert_eq!(json["summary"]["executed"], 1);
    assert_eq!(json["summary"]["successful"], 1);
    assert_eq!(json["summary"]["failed"], 0);
    assert!(json["results"].is_array());
}

#[test]
fn test_add_runner_results_to_json_real() {
    let mut harness = harness("bar::harness_fail", "src/main.rs");
    harness.original_start_line = 5;
    harness.original_end_line = 15;
    let verification_result = failure_result(Err(ExitStatus::Other(42)));

    let harness_result = HarnessResult { harness: &harness, result: verification_result };

    let mut handler = JsonHandler::new(None);
    add_runner_results_to_json(&mut handler, &[harness_result], 1, "Failed");

    let summary = &handler.data["verification_results"]["summary"];
    assert_eq!(summary["status"], "Failed");
    assert_eq!(summary["failed"], 1);
}
