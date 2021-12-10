//! Snapshot command

use crate::cmd::{
    test,
    test::{Test, TestOutcome},
    Cmd,
};
use once_cell::sync::Lazy;
use regex::Regex;
use std::{
    collections::HashMap,
    fmt::Write,
    fs,
    io::{self, BufRead},
    path::{Path, PathBuf},
    str::FromStr,
};
use structopt::StructOpt;

/// A regex that matches a basic snapshot entry like
/// `testDeposit() (gas: 58804)`
pub static RE_BASIC_SNAPSHOT_ENTRY: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"(?P<sig>(\w+)\s*\((.*?)\))\s*\((gas:)?\s*(?P<gas>\d+)\)").unwrap());

#[derive(Debug, Clone, StructOpt)]
pub struct SnapshotArgs {
    /// All test arguments are supported
    #[structopt(flatten)]
    test: test::TestArgs,
    /// Additional configs for test results
    #[structopt(flatten)]
    config: SnapshotConfig,
    #[structopt(
        help = "Compare against a snapshot and display changes from the snapshot. Takes an optional snapshot file, [default: .gas-snapshot]",
        long
    )]
    diff: Option<Option<PathBuf>>,
    #[structopt(help = "How to format the output.", long)]
    format: Option<Format>,
    #[structopt(help = "Output file for the snapshot.", default_value = ".gas-snapshot", long)]
    snap: PathBuf,
}

impl Cmd for SnapshotArgs {
    type Output = ();

    fn run(self) -> eyre::Result<()> {
        let outcome = self.test.run()?;
        outcome.ensure_ok()?;
        let tests = self.config.apply(outcome);

        match self.diff {
            Some(Some(snap)) => {
                let snaps = read_snapshot(snap)?;
                diff(tests, snaps)?;
            }
            Some(None) => {
                let snaps = read_snapshot(self.snap)?;
                diff(tests, snaps)?;
            }
            _ => {
                write_to_snapshot_file(&tests, self.snap)?;
            }
        }
        Ok(())
    }
}

// TODO implement pretty tables
#[derive(Debug, Clone)]
pub enum Format {
    Table,
}

impl FromStr for Format {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "t" | "table" => Ok(Format::Table),
            _ => Err(format!("Unrecognized format `{}`", s)),
        }
    }
}

#[derive(Debug, Clone, StructOpt)]
pub struct CheckSnapshotArgs {
    #[structopt(
        help = " Input gas snapshot file to compare against.",
        default_value = ".gas_snapshot",
        short,
        long
    )]
    input: PathBuf,
    #[structopt(flatten)]
    config: SnapshotConfig,
}

impl Cmd for CheckSnapshotArgs {
    type Output = ();

    fn run(self) -> eyre::Result<()> {
        dbg!(self);
        todo!()
    }
}

/// Additional filters that can be applied on the test results
#[derive(Debug, Clone, StructOpt, Default)]
struct SnapshotConfig {
    #[structopt(help = "sort results by ascending gas used.", long)]
    asc: bool,
    #[structopt(help = "sort results by descending gas used.", conflicts_with = "asc", long)]
    desc: bool,
    #[structopt(help = "Only include tests that used more gas that the given amount.", long)]
    min: Option<u64>,
    #[structopt(help = "Only include tests that used less gas that the given amount.", long)]
    max: Option<u64>,
}

impl SnapshotConfig {
    fn is_in_gas_range(&self, gas_used: u64) -> bool {
        if let Some(min) = self.min {
            if gas_used < min {
                return false
            }
        }
        if let Some(max) = self.max {
            if gas_used > max {
                return false
            }
        }
        true
    }

    fn apply(&self, outcome: TestOutcome) -> Vec<Test> {
        let mut tests = outcome
            .into_tests()
            .filter_map(|test| test.gas_used().map(|gas| (test, gas)))
            .filter(|(_test, gas)| self.is_in_gas_range(*gas))
            .map(|(test, _)| test)
            .collect::<Vec<_>>();

        if self.asc {
            tests.sort_by_key(|a| a.gas_used());
        } else if self.desc {
            tests.sort_by_key(|b| std::cmp::Reverse(b.gas_used()))
        }

        tests
    }
}

/// A general entry in a snapshot file
///
/// Has the form `<signature>(gas:? 40181)`
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct SnapshotEntry {
    pub signature: String,
    pub gas_used: u64,
}

impl FromStr for SnapshotEntry {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        RE_BASIC_SNAPSHOT_ENTRY
            .captures(s)
            .and_then(|cap| {
                cap.name("sig").and_then(|sig| {
                    cap.name("gas").map(|gas| SnapshotEntry {
                        signature: sig.as_str().to_string(),
                        gas_used: gas.as_str().parse().unwrap(),
                    })
                })
            })
            .ok_or_else(|| format!("Could not extract Snapshot Entry for {}", s))
    }
}

/// Reads a list of snapshot entries from a snapshot file
fn read_snapshot(path: impl AsRef<Path>) -> eyre::Result<Vec<SnapshotEntry>> {
    let mut entries = Vec::new();
    for line in io::BufReader::new(fs::File::open(path)?).lines() {
        entries
            .push(SnapshotEntry::from_str(line?.as_str()).map_err(|err| eyre::eyre!("{}", err))?);
    }
    Ok(entries)
}

/// Writes a series of tests to a snapshot file
fn write_to_snapshot_file(tests: &[Test], path: impl AsRef<Path>) -> eyre::Result<()> {
    let mut out = String::new();
    for test in tests {
        if let Some(gas) = test.gas_used() {
            writeln!(out, "{} (gas: {})", test.signature, gas)?;
        }
    }
    Ok(fs::write(path, out)?)
}

/// A Snapshot entry diff
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct SnapshotDiff {
    pub signature: String,
    pub source_gas_used: u64,
    pub target_gas_used: u64,
}

impl SnapshotDiff {
    /// Returns the gas diff
    ///
    /// `> 0` if the source used more gas
    /// `< 0` if the source used more gas
    fn gas_change(&self) -> i128 {
        self.source_gas_used as i128 - self.target_gas_used as i128
    }

    /// Determines the percentage change
    fn gas_diff(&self) -> f64 {
        self.gas_change() as f64 / self.target_gas_used as f64
    }
}

/// Compare the set of tests with an existing snapshot
fn diff(tests: Vec<Test>, snaps: Vec<SnapshotEntry>) -> eyre::Result<()> {
    let snaps = snaps.into_iter().map(|s| (s.signature, s.gas_used)).collect::<HashMap<_, _>>();
    let mut diffs = Vec::with_capacity(tests.len());
    for test in tests.into_iter().filter(|t| t.gas_used().is_some()) {
        let target_gas_used = snaps.get(&test.signature).cloned().ok_or_else(|| {
            eyre::eyre!(
                "No matching snapshot entry found for \"{}\" in snapshot file",
                test.signature
            )
        })?;

        diffs.push(SnapshotDiff {
            source_gas_used: test.gas_used().unwrap(),
            signature: test.signature,
            target_gas_used,
        });
    }
    let mut overall_gas_change = 0i128;
    let mut overall_gas_diff = 0f64;

    diffs.sort_by(|a, b| a.gas_diff().partial_cmp(&b.gas_diff()).unwrap());

    for diff in diffs {
        let gas_change = diff.gas_change();
        overall_gas_change += gas_change;
        let gas_diff = diff.gas_diff();
        overall_gas_diff += gas_diff;
        println!("{} (gas: {} ({:.3}%)) ", diff.signature, gas_change, gas_diff);
    }

    let is_pos = if overall_gas_change > 0 { "+" } else { "" };
    println!(
        "Overall gas change: {}{} ({}{:.3}%)",
        is_pos, overall_gas_change, is_pos, overall_gas_diff
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn can_parse_snapshot_entry() {
        let s = "deposit() (gas: 7222)";
        let entry = SnapshotEntry::from_str(s).unwrap();
        assert_eq!(entry, SnapshotEntry { signature: "deposit()".to_string(), gas_used: 7222 });
    }
}
