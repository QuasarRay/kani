// Copyright Kani Contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT
//! This module handles outputting the result for the list subcommand

use std::{
    collections::BTreeSet,
    fs::File,
    io::{BufWriter, Write},
    path::Path,
};

use crate::{
    args::list_args::Format,
    list::{ListMetadata, merge_list_metadata},
    version::KANI_VERSION,
};
use anyhow::Result;
use cgp::prelude::*;
use comfy_table::Table as PrettyTable;
use comprende::c;
use serde_json::json;
use to_markdown_table::MarkdownTable;

// Represents the version of our JSON file format.
// Increment this version (according to semantic versioning rules) whenever the JSON output format changes.
const FILE_VERSION: &str = "0.1";
const OUTPUT_FILENAME: &str = "kani-list";

struct PrettyOutput;
struct MarkdownOutput;

#[cgp_component(TableRenderer)]
trait CanRenderTable {
    fn render_table(&self, header: Vec<String>, rows: Vec<Vec<String>>) -> Result<String>;
}

#[cgp_impl(new RenderPrettyTable)]
impl TableRenderer {
    fn render_table(&self, header: Vec<String>, rows: Vec<Vec<String>>) -> Result<String> {
        let mut table = PrettyTable::new();
        table.set_header(header).add_rows(rows);
        Ok(table.to_string())
    }
}

#[cgp_impl(new RenderMarkdownTable)]
impl TableRenderer {
    fn render_table(&self, header: Vec<String>, rows: Vec<Vec<String>>) -> Result<String> {
        Ok(MarkdownTable::new(Some(header), rows)?.to_string())
    }
}

delegate_and_check_components! {
    PrettyOutput {
        TableRendererComponent: RenderPrettyTable,
    }
}

delegate_and_check_components! {
    MarkdownOutput {
        TableRendererComponent: RenderMarkdownTable,
    }
}

/// Output the results of the list subcommand.
pub fn output_list_results(
    list_metadata: BTreeSet<ListMetadata>,
    format: Format,
    quiet: bool,
) -> Result<()> {
    match format {
        Format::Pretty => pretty(list_metadata),
        Format::Markdown => markdown(list_metadata, quiet),
        Format::Json => json(list_metadata, quiet),
    }
}

/// Construct the "Contracts" and "Standard Harnesses" tables.
fn construct_output<Renderer: CanRenderTable>(
    renderer: &Renderer,
    list_metadata: BTreeSet<ListMetadata>,
) -> Result<(String, String)> {
    const CONTRACTS_SECTION: &str = "Contracts:";
    const NO_CONTRACTS_MSG: &str = "No contracts or contract harnesses found.";
    const HARNESSES_SECTION: &str = "Standard Harnesses (#[kani::proof]):";
    const NO_HARNESSES_MSG: &str = "No standard harnesses found.";

    let contract_table = if list_metadata.iter().all(|md| md.contracted_functions.is_empty()) {
        None
    } else {
        let (header, rows) = construct_contracts_table(&list_metadata);
        Some(renderer.render_table(header, rows)?)
    };
    let (header, rows) = construct_standard_table(&list_metadata);
    let standard_table = Some(renderer.render_table(header, rows)?);

    Ok((
        format_results(contract_table, CONTRACTS_SECTION, NO_CONTRACTS_MSG),
        format_results(standard_table, HARNESSES_SECTION, NO_HARNESSES_MSG),
    ))
}

/// Print results to the terminal.
fn pretty(list_metadata: BTreeSet<ListMetadata>) -> Result<()> {
    let (contract_output, standard_output) = construct_output(&PrettyOutput, list_metadata)?;
    println!("{contract_output}");
    println!("{standard_output}");
    Ok(())
}

/// Output results to a Markdown file.
fn markdown(list_metadata: BTreeSet<ListMetadata>, quiet: bool) -> Result<()> {
    let (contract_output, standard_output) = construct_output(&MarkdownOutput, list_metadata)?;
    let out_path = Path::new(OUTPUT_FILENAME).with_extension("md");
    let mut out_file = File::create(&out_path).unwrap();
    out_file.write_all(contract_output.as_bytes()).unwrap();
    out_file.write_all(standard_output.as_bytes()).unwrap();
    if !quiet {
        println!("Wrote list results to {}", std::fs::canonicalize(&out_path)?.display());
    }
    Ok(())
}

/// Output results as a JSON file.
fn json(list_metadata: BTreeSet<ListMetadata>, quiet: bool) -> Result<()> {
    let out_path = Path::new(OUTPUT_FILENAME).with_extension("json");
    let out_file = File::create(&out_path).unwrap();
    let writer = BufWriter::new(out_file);

    let combined_md = merge_list_metadata(list_metadata);

    let json_obj = json!({
        "kani-version": KANI_VERSION,
        "file-version": FILE_VERSION,
        "standard-harnesses": combined_md.standard_harnesses,
        "contract-harnesses": combined_md.contract_harnesses,
        "contracts": combined_md.contracted_functions,
        "totals": {
            "standard-harnesses": combined_md.standard_harnesses_count,
            "contract-harnesses": combined_md.contract_harnesses_count,
            "functions-under-contract": combined_md.contracted_functions.len(),
        }
    });

    serde_json::to_writer_pretty(writer, &json_obj)?;

    if !quiet {
        println!("Wrote list results to {}", std::fs::canonicalize(out_path)?.display());
    }

    Ok(())
}

/// Construct the rows for the table of contracts information.
/// Returns a tuple of the table header and the rows.
fn construct_contracts_table(
    list_metadata: &BTreeSet<ListMetadata>,
) -> (Vec<String>, Vec<Vec<String>>) {
    const NO_HARNESSES_MSG: &str = "NONE";
    const CRATE_NAME: &str = "Crate";
    const FUNCTION_HEADER: &str = "Function";
    const CONTRACT_HARNESSES_HEADER: &str = "Contract Harnesses (#[kani::proof_for_contract])";
    const TOTALS_HEADER: &str = "Total";

    let header = vec![
        String::new(),
        CRATE_NAME.to_string(),
        FUNCTION_HEADER.to_string(),
        CONTRACT_HARNESSES_HEADER.to_string(),
    ];
    let mut rows = c![
        vec![
            String::new(),
            crate_md.crate_name.to_string(),
            cf.function.clone(),
            if cf.harnesses.is_empty() {
                NO_HARNESSES_MSG.to_string()
            } else {
                cf.harnesses.join(", ")
            },
        ]
        for crate_md in list_metadata
        for cf in &crate_md.contracted_functions
    ];
    let functions_under_contract_total = rows.len();
    let contract_harnesses_total: usize =
        list_metadata.iter().map(|md| md.contract_harnesses_count).sum();

    rows.push(vec![
        TOTALS_HEADER.to_string(),
        String::new(),
        functions_under_contract_total.to_string(),
        contract_harnesses_total.to_string(),
    ]);
    (header, rows)
}

fn construct_standard_table(
    list_metadata: &BTreeSet<ListMetadata>,
) -> (Vec<String>, Vec<Vec<String>>) {
    const CRATE_NAME: &str = "Crate";
    const HARNESS_HEADER: &str = "Harness";
    const TOTALS_HEADER: &str = "Total";

    let header = vec![String::new(), CRATE_NAME.to_string(), HARNESS_HEADER.to_string()];
    let mut rows = c![
        vec![String::new(), crate_md.crate_name.to_string(), harness.to_string()]
        for crate_md in list_metadata
        for harnesses in crate_md.standard_harnesses.values()
        for harness in harnesses
    ];
    let total = rows.len();

    rows.push(vec![TOTALS_HEADER.to_string(), String::new(), total.to_string()]);
    (header, rows)
}

fn format_results(table: Option<String>, section_name: &str, absent_name: &str) -> String {
    format!("\n{section_name}\n{}", table.unwrap_or_else(|| absent_name.to_string()))
}
