use comfy_table::{
    modifiers::UTF8_ROUND_CORNERS, Attribute, Cell, CellAlignment, Color, Row, Table,
};
use foundry_common::shell;
use core::fmt;
use std::{collections::BTreeMap, time::Duration};
use yansi::Paint;
use serde::{Deserialize, Serialize};
use gambit::{Mutant};


#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum MutantTestStatus {
    Killed,
    Survived,
    #[default]
    Equivalent
}

impl fmt::Display for MutantTestStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            MutantTestStatus::Killed => Paint::green("[KILLED]").fmt(f),
            MutantTestStatus::Survived => Paint::red("[SURVIVED]").fmt(f),
            MutantTestStatus::Equivalent => Paint::yellow("[EQUIVALENT]").fmt(f)
        }
    }
}

#[derive(Debug, Clone)]
pub struct MutantTestResult {
    pub duration: Duration,
    pub mutant: Mutant,
    status: MutantTestStatus
}


impl MutantTestResult {
    pub fn new(
        duration: Duration,
        mutant: Mutant,
        status: MutantTestStatus
    ) -> Self {
        Self { duration, mutant, status }
    }

    pub fn killed(&self) -> bool {
        matches!(self.status, MutantTestStatus::Killed)
    }

    pub fn survived(&self) -> bool {
        matches!(self.status, MutantTestStatus::Survived)
    }

    pub fn equivalent(&self) -> bool {
        matches!(self.status, MutantTestStatus::Equivalent)
    }

    pub fn diff(&self) -> String {
        "".into()
    }
}

/// Results and duration for mutation tests for a contract
#[derive(Debug, Clone)]
pub struct MutationTestSuiteResult {
    /// Total duration of the mutation tests run for this contract
    pub duration: Duration,
    /// Individual mutation test results. `file_name -> MutationTestResult`
    mutation_test_results: Vec<MutantTestResult>,
}

impl MutationTestSuiteResult {
    pub fn new(
       results: Vec<MutantTestResult>
    ) -> Self {
        let duration: Duration = results.iter()
            .fold( Duration::from_secs(0),|init: Duration, e| init + e.duration);

        Self { duration, mutation_test_results: results }
    }

    pub fn killed(&self) -> impl Iterator<Item = &MutantTestResult>  {
        self.mutation_test_results().filter(
            |result| result.killed() 
        )
    }

    pub fn survived(&self) -> impl Iterator<Item = &MutantTestResult>  {
        self.mutation_test_results().filter(
            |result| result.survived()
        )
    }

    pub fn equivalent(&self) -> impl Iterator<Item = &MutantTestResult>  {
        self.mutation_test_results().filter(
            |result| result.equivalent()
        )
    }

    pub fn mutation_test_results(&self) -> impl Iterator<Item = &MutantTestResult> {
        self.mutation_test_results.iter()
    }

    pub fn is_empty(&self) -> bool {
        self.mutation_test_results.is_empty()
    }

    pub fn len(&self) -> usize {
        self.mutation_test_results.len()
    }
}

/// Represents the bundled results of all tests
#[derive(Clone, Debug)]
pub struct MutationTestOutcome {
    /// Whether failures are allowed
    /// This enables to exit early
    pub allow_failure: bool,

    // this would be Contract -> SuiteResult
    pub test_suite_result: BTreeMap<String, MutationTestSuiteResult>
}

impl MutationTestOutcome {
    pub fn new(
        allow_failure: bool,
        test_suite_result: BTreeMap<String, MutationTestSuiteResult>
    ) -> Self {
        Self {
            allow_failure,
            test_suite_result
        }
    }

    /// Total duration for tests
    pub fn duration(&self) -> Duration {
        self.test_suite_result.values().map(|suite| suite.duration)
            .fold(Duration::from_secs(0), |acc, duration| acc + duration)
    }

    /// Iterator over all killed mutation tests
    pub fn killed(&self) -> impl Iterator<Item = &MutantTestResult>  {
        self.results().filter(|result| result.killed())
    }

    /// Iterator over all surviving mutation tests
    pub fn survived(&self) -> impl Iterator<Item = &MutantTestResult>  {
        self.results().filter(|result| result.survived())
    }

    /// Iterator over all equivalent mutation tests
    pub fn equivalent(&self) -> impl Iterator<Item = &MutantTestResult>   {
        self.results().filter(|result| result.equivalent())
    }

    /// Iterator over all mutation tests and their names
    pub fn results(&self) -> impl Iterator<Item = &MutantTestResult> {
        self.test_suite_result.values().flat_map(|suite| suite.mutation_test_results())
    }

    pub fn summary(&self) -> String {
        let survived = self.survived().count();
        let result = if survived == 0 { Paint::green("ok") } else { Paint::red("FAILED") };
        format!(
            "Mutation Test result: {}. {} killed; {} survived; {} equivalent; finished in {:.2?}",
            result,
            Paint::green(self.killed().count()),
            Paint::red(survived),
            Paint::yellow(self.equivalent().count()),
            self.duration()
        )
    }

    /// Checks if there is any surviving mutations and failures are disallowed
    pub fn ensure_ok(&self) -> eyre::Result<()> {
        let survived = self.survived().count();

        if self.allow_failure || survived == 0 {
            return Ok(())
        }

        if !shell::verbosity().is_normal() {
            // skip printing and exit early
            std::process::exit(1);
        }

        println!();
        println!("Surviving Mutations:");

        for (file_name, suite_result) in self.test_suite_result.iter() {
            let survived = suite_result.survived().count();
            if survived == 0 {
                continue
            }

            let term = if survived > 1 { "mutations" } else { "mutation" };
            println!("Encountered {} surviving {term} in {}", survived, file_name);
            // @TODO println surviving diff
        }

        println!(
            "Encountered a total of {} surviving mutations, {} mutations killed",
            Paint::red(survived.to_string()),
            Paint::green(self.killed().count().to_string())
        );
        std::process::exit(1);
    }

}

pub struct MutationTestSummaryReporter {
    /// The mutation test summary table.
    pub(crate) table: Table,
    pub(crate) is_detailed: bool,
}

impl MutationTestSummaryReporter {

    pub(crate) fn new(is_detailed: bool) -> Self {
        let mut table = Table::new();
        table.apply_modifier(UTF8_ROUND_CORNERS);
        let mut row = Row::from(vec![
            Cell::new("Contract")
                .set_alignment(CellAlignment::Left)
                .add_attribute(Attribute::Bold),
            Cell::new("Killed")
                .set_alignment(CellAlignment::Center)
                .add_attribute(Attribute::Bold)
                .fg(Color::Green),
            Cell::new("Survived")
                .set_alignment(CellAlignment::Center)
                .add_attribute(Attribute::Bold)
                .fg(Color::Red),
            Cell::new("Equivalent")
                .set_alignment(CellAlignment::Center)
                .add_attribute(Attribute::Bold)
                .fg(Color::Yellow),
        ]);
        
        if is_detailed {
            // row.add_cell(
            //     Cell::new("Diff")
            //         .set_alignment(CellAlignment::Center)
            //         .add_attribute(Attribute::Bold),
            // );
            row.add_cell(
                Cell::new("Duration")
                    .set_alignment(CellAlignment::Center)
                    .add_attribute(Attribute::Bold),
            );
        }

        table.set_header(row);

        Self { table , is_detailed }
    }


    pub fn print_summary(&mut self, mut mutation_test_outcome: &MutationTestOutcome) {
        for (contract_name, suite_result) in mutation_test_outcome.test_suite_result.iter() {
            let mut row = Row::new();

            let contract_title: String;
            if let Some(result) = suite_result.mutation_test_results.first() {
                contract_title = format!(
                    "{}:{}",
                    result.mutant.source.filename_as_str(),
                    contract_name
                );
            } else {
                contract_title = contract_name.to_string();
            }

            let file_cell = Cell::new(contract_title).set_alignment(CellAlignment::Left);
            row.add_cell(file_cell);

            let killed = suite_result.killed().count();
            let survived = suite_result.survived().count();
            let equivalent = suite_result.equivalent().count();

            let mut killed_cell = Cell::new(killed).set_alignment(CellAlignment::Center);
            let mut survived_cell = Cell::new(survived).set_alignment(CellAlignment::Center);
            let mut equivalent_cell = Cell::new(equivalent).set_alignment(CellAlignment::Center);

            if killed > 0 {
                killed_cell = killed_cell.fg(Color::Green);
            }
            row.add_cell(killed_cell);

            if survived > 0 {
                survived_cell = survived_cell.fg(Color::Red);
            }
            row.add_cell(survived_cell);

            if equivalent > 0 {
                equivalent_cell = equivalent_cell.fg(Color::Yellow);
            }
            row.add_cell(equivalent_cell);

            if self.is_detailed {
                row.add_cell(Cell::new(format!("{:.2?}", suite_result.duration).to_string()));
            }
            self.table.add_row(row);
        }
        println!("\n{}", self.table);
    }
}