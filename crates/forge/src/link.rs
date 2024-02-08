use alloy_primitives::{Address, Bytes};
use eyre::Result;
use foundry_compilers::{artifacts::Libraries, contracts::ArtifactContracts, Artifact, ArtifactId};
use semver::Version;
use std::{collections::HashSet, path::PathBuf, str::FromStr};

/// Helper method to convert [ArtifactId] to the format in which libraries are stored in [Libraries]
/// object.
///
/// Strips project root path from source file path.
fn convert_artifact_id_to_lib_path(id: &ArtifactId, root_path: &PathBuf) -> (PathBuf, String) {
    let path = id.source.strip_prefix(root_path).unwrap_or(&id.source);
    // name is either {LibName} or {LibName}.{version}
    let name = id.name.split('.').next().unwrap();

    (path.to_path_buf(), name.to_owned())
}

/// Finds an [ArtifactId] object in the given [ArtifactContracts] keys which corresponds to the
/// library path in the form of "./path/to/Lib.sol:Lib"
///
/// Optionally accepts solc version, and if present, only compares artifacts with given version.
fn find_artifact_id_by_library_path<'a>(
    contracts: &'a ArtifactContracts,
    file: &String,
    name: &String,
    version: Option<&Version>,
    root_path: &PathBuf,
) -> &'a ArtifactId {
    for id in contracts.keys() {
        if let Some(version) = version {
            if id.version != *version {
                continue;
            }
        }
        let (artifact_path, artifact_name) = convert_artifact_id_to_lib_path(id, root_path);

        if artifact_name == *name && artifact_path == PathBuf::from(file) {
            return id;
        }
    }

    panic!("artifact not found for library {file} {name}");
}

/// Performs DFS on the graph of link references, and populates `deps` with all found libraries.
pub fn collect_dependencies<'a>(
    target: &'a ArtifactId,
    contracts: &'a ArtifactContracts,
    deps: &mut HashSet<&'a ArtifactId>,
    root_path: &PathBuf,
) {
    let references = contracts.get(target).unwrap().all_link_references();
    for (file, libs) in &references {
        for contract in libs.keys() {
            let id = find_artifact_id_by_library_path(
                contracts,
                file,
                contract,
                Some(&target.version),
                root_path,
            );
            if deps.insert(id) {
                collect_dependencies(id, contracts, deps, root_path);
            }
        }
    }
}

/// Links given artifacts with given library addresses.
///
/// Artifacts returned by this function may still be partially unlinked if some of their
/// dependencies weren't present in `libraries`.
pub fn link(contracts: &ArtifactContracts, libraries: &Libraries) -> Result<ArtifactContracts> {
    contracts
        .iter()
        .map(|(id, contract)| {
            let mut contract = contract.clone();

            for (file, libs) in &libraries.libs {
                for (name, address) in libs {
                    let address = Address::from_str(address)?;
                    if let Some(bytecode) = contract.bytecode.as_mut() {
                        bytecode.link(file.to_string_lossy(), name, address);
                    }
                    if let Some(deployed_bytecode) =
                        contract.deployed_bytecode.as_mut().and_then(|b| b.bytecode.as_mut())
                    {
                        deployed_bytecode.link(file.to_string_lossy(), name, address);
                    }
                }
            }
            Ok((id.clone(), contract))
        })
        .collect()
}

/// Output of the `link_with_nonce_or_address`
pub struct LinkOutput {
    /// [ArtifactContracts] object containing all artifacts linked with known libraries
    /// It is guaranteed to contain `target` and all it's dependencies fully linked, and any other
    /// contract may still be partially unlinked.
    pub contracts: ArtifactContracts,
    /// Resulted library addresses. Contains both user-provided and newly deployed libraries.
    /// It will always contain library paths with stripped path prefixes.
    pub libraries: Libraries,
    /// Vector of libraries that need to be deployed from sender address.
    /// The order in which they appear in the vector is the order in which they should be deployed.
    pub libs_to_deploy: Vec<Bytes>,
}

/// Links given artifact with either given library addresses or address computed from sender and
/// nonce.
///
/// Each key in `libraries` should either be a global path or relative to project root. All
/// remappings should be resolved.
pub fn link_with_nonce_or_address<'a>(
    contracts: &'a ArtifactContracts,
    libraries: Libraries,
    sender: Address,
    mut nonce: u64,
    target: &'a ArtifactId,
    root_path: &PathBuf,
) -> Result<LinkOutput> {
    // Library paths in `link_references` keys are always stripped, so we have to strip
    // user-provided paths to be able to match them correctly.
    let mut libraries = libraries.with_stripped_file_prefixes(root_path);

    let mut needed_libraries = HashSet::new();
    collect_dependencies(target, contracts, &mut needed_libraries, root_path);

    let mut libs_to_deploy = Vec::new();

    // If `libraries` does not contain needed dependency, compute its address and add to
    // `libs_to_deploy`.
    for id in needed_libraries {
        let (lib_path, lib_name) = convert_artifact_id_to_lib_path(id, root_path);

        libraries.libs.entry(lib_path).or_default().entry(lib_name).or_insert_with(|| {
            let address = sender.create(nonce);
            libs_to_deploy.push((id, address));
            nonce += 1;

            address.to_checksum(None)
        });
    }

    // Link contracts
    let contracts = link(contracts, &libraries)?;

    // Collect bytecodes for `libs_to_deploy`, as we have them linked now.
    let libs_to_deploy = libs_to_deploy
        .into_iter()
        .map(|(id, _)| contracts.get(id).unwrap().get_bytecode_bytes().unwrap().into_owned())
        .collect();

    Ok(LinkOutput { contracts, libraries, libs_to_deploy })
}

#[cfg(test)]
mod tests {
    use std::{collections::HashMap, path::PathBuf};

    use super::*;
    use foundry_compilers::{Project, ProjectPathsConfig};

    struct LinkerTest {
        project: Project,
        contracts: ArtifactContracts,
        dependency_assertions: HashMap<String, Vec<(String, u64, Address)>>,
    }

    impl LinkerTest {
        fn new(path: impl Into<PathBuf>) -> Self {
            let path = path.into();
            let paths = ProjectPathsConfig::builder()
                .root("../../testdata/linking")
                .lib("../../testdata/lib")
                .sources(path.clone())
                .tests(path)
                .build()
                .unwrap();

            let project =
                Project::builder().paths(paths).ephemeral().no_artifacts().build().unwrap();
            let contracts = project
                .compile()
                .unwrap()
                .with_stripped_file_prefixes(project.root())
                .into_artifacts()
                .map(|(id, c)| (id, c.into_contract_bytecode()))
                .collect::<ArtifactContracts>();

            Self { project, contracts, dependency_assertions: HashMap::new() }
        }

        fn assert_dependencies(
            mut self,
            artifact_id: String,
            deps: Vec<(String, u64, Address)>,
        ) -> Self {
            self.dependency_assertions.insert(artifact_id, deps);
            self
        }

        fn test_with_sender_and_nonce(self, sender: Address, initial_nonce: u64) {
            for id in self.contracts.keys() {
                let identifier = id.identifier();

                // Skip ds-test as it always has no dependencies etc. (and the path is outside root
                // so is not sanitized)
                if identifier.contains("DSTest") {
                    continue;
                }

                let LinkOutput { libs_to_deploy, libraries, .. } = link_with_nonce_or_address(
                    &self.contracts,
                    Default::default(),
                    sender,
                    initial_nonce,
                    id,
                    self.project.root(),
                )
                .expect("Linking failed");

                let assertions = self
                    .dependency_assertions
                    .get(&identifier)
                    .unwrap_or_else(|| panic!("Unexpected artifact: {identifier}"));

                let expected_libs =
                    assertions.iter().map(|(identifier, _, _)| identifier).collect::<HashSet<_>>();

                assert_eq!(
                    libs_to_deploy.len(),
                    expected_libs.len(),
                    "artifact {identifier} has more/less dependencies than expected ({} vs {}): {:#?}",
                    libs_to_deploy.len(),
                    assertions.len(),
                    libs_to_deploy
                );

                let identifiers = libraries
                    .libs
                    .iter()
                    .flat_map(|(file, libs)| {
                        libs.iter().map(|(name, _)| format!("{}:{}", file.to_string_lossy(), name))
                    })
                    .collect::<Vec<_>>();

                for lib in expected_libs {
                    assert!(identifiers.contains(lib));
                }
            }
        }
    }

    #[test]
    fn link_simple() {
        LinkerTest::new("../../testdata/linking/simple")
            .assert_dependencies("simple/Simple.t.sol:Lib".to_string(), vec![])
            .assert_dependencies(
                "simple/Simple.t.sol:LibraryConsumer".to_string(),
                vec![(
                    "simple/Simple.t.sol:Lib".to_string(),
                    1,
                    Address::from_str("0x5a443704dd4b594b382c22a083e2bd3090a6fef3").unwrap(),
                )],
            )
            .assert_dependencies(
                "simple/Simple.t.sol:SimpleLibraryLinkingTest".to_string(),
                vec![(
                    "simple/Simple.t.sol:Lib".to_string(),
                    1,
                    Address::from_str("0x5a443704dd4b594b382c22a083e2bd3090a6fef3").unwrap(),
                )],
            )
            .test_with_sender_and_nonce(Address::default(), 1);
    }

    #[test]
    fn link_nested() {
        LinkerTest::new("../../testdata/linking/nested")
            .assert_dependencies("nested/Nested.t.sol:Lib".to_string(), vec![])
            .assert_dependencies(
                "nested/Nested.t.sol:NestedLib".to_string(),
                vec![(
                    "nested/Nested.t.sol:Lib".to_string(),
                    1,
                    Address::from_str("0x5a443704dd4b594b382c22a083e2bd3090a6fef3").unwrap(),
                )],
            )
            .assert_dependencies(
                "nested/Nested.t.sol:LibraryConsumer".to_string(),
                vec![
                    // Lib shows up here twice, because the linker sees it twice, but it should
                    // have the same address and nonce.
                    (
                        "nested/Nested.t.sol:Lib".to_string(),
                        1,
                        Address::from_str("0x5a443704dd4b594b382c22a083e2bd3090a6fef3").unwrap(),
                    ),
                    (
                        "nested/Nested.t.sol:Lib".to_string(),
                        1,
                        Address::from_str("0x5a443704dd4b594b382c22a083e2bd3090a6fef3").unwrap(),
                    ),
                    (
                        "nested/Nested.t.sol:NestedLib".to_string(),
                        2,
                        Address::from_str("0x47e9fbef8c83a1714f1951f142132e6e90f5fa5d").unwrap(),
                    ),
                ],
            )
            .assert_dependencies(
                "nested/Nested.t.sol:NestedLibraryLinkingTest".to_string(),
                vec![
                    (
                        "nested/Nested.t.sol:Lib".to_string(),
                        1,
                        Address::from_str("0x5a443704dd4b594b382c22a083e2bd3090a6fef3").unwrap(),
                    ),
                    (
                        "nested/Nested.t.sol:Lib".to_string(),
                        1,
                        Address::from_str("0x5a443704dd4b594b382c22a083e2bd3090a6fef3").unwrap(),
                    ),
                    (
                        "nested/Nested.t.sol:NestedLib".to_string(),
                        2,
                        Address::from_str("0x47e9fbef8c83a1714f1951f142132e6e90f5fa5d").unwrap(),
                    ),
                ],
            )
            .test_with_sender_and_nonce(Address::default(), 1);
    }

    /// This test ensures that complicated setups with many libraries, some of which are referenced
    /// in more than one place, result in correct linking.
    ///
    /// Each `assert_dependencies` should be considered in isolation, i.e. read it as "if I wanted
    /// to deploy this contract, I would have to deploy the dependencies in this order with this
    /// nonce".
    ///
    /// A library may show up more than once, but it should *always* have the same nonce and address
    /// with respect to the single `assert_dependencies` call. There should be no gaps in the nonce
    /// otherwise, i.e. whenever a new dependency is encountered, the nonce should be a single
    /// increment larger than the previous largest nonce.
    #[test]
    fn link_duplicate() {
        LinkerTest::new("../../testdata/linking/duplicate")
            .assert_dependencies("duplicate/Duplicate.t.sol:A".to_string(), vec![])
            .assert_dependencies("duplicate/Duplicate.t.sol:B".to_string(), vec![])
            .assert_dependencies(
                "duplicate/Duplicate.t.sol:C".to_string(),
                vec![(
                    "duplicate/Duplicate.t.sol:A".to_string(),
                    1,
                    Address::from_str("0x5a443704dd4b594b382c22a083e2bd3090a6fef3").unwrap(),
                )],
            )
            .assert_dependencies(
                "duplicate/Duplicate.t.sol:D".to_string(),
                vec![(
                    "duplicate/Duplicate.t.sol:B".to_string(),
                    1,
                    Address::from_str("0x5a443704dd4b594b382c22a083e2bd3090a6fef3").unwrap(),
                )],
            )
            .assert_dependencies(
                "duplicate/Duplicate.t.sol:E".to_string(),
                vec![
                    (
                        "duplicate/Duplicate.t.sol:A".to_string(),
                        1,
                        Address::from_str("0x5a443704dd4b594b382c22a083e2bd3090a6fef3").unwrap(),
                    ),
                    (
                        "duplicate/Duplicate.t.sol:C".to_string(),
                        2,
                        Address::from_str("0x47e9fbef8c83a1714f1951f142132e6e90f5fa5d").unwrap(),
                    ),
                ],
            )
            .assert_dependencies(
                "duplicate/Duplicate.t.sol:LibraryConsumer".to_string(),
                vec![
                    (
                        "duplicate/Duplicate.t.sol:A".to_string(),
                        1,
                        Address::from_str("0x5a443704dd4b594b382c22a083e2bd3090a6fef3").unwrap(),
                    ),
                    (
                        "duplicate/Duplicate.t.sol:B".to_string(),
                        2,
                        Address::from_str("0x47e9fbef8c83a1714f1951f142132e6e90f5fa5d").unwrap(),
                    ),
                    (
                        "duplicate/Duplicate.t.sol:A".to_string(),
                        1,
                        Address::from_str("0x5a443704dd4b594b382c22a083e2bd3090a6fef3").unwrap(),
                    ),
                    (
                        "duplicate/Duplicate.t.sol:C".to_string(),
                        3,
                        Address::from_str("0x8be503bcded90ed42eff31f56199399b2b0154ca").unwrap(),
                    ),
                    (
                        "duplicate/Duplicate.t.sol:B".to_string(),
                        2,
                        Address::from_str("0x47e9fbef8c83a1714f1951f142132e6e90f5fa5d").unwrap(),
                    ),
                    (
                        "duplicate/Duplicate.t.sol:D".to_string(),
                        4,
                        Address::from_str("0x47c5e40890bce4a473a49d7501808b9633f29782").unwrap(),
                    ),
                    (
                        "duplicate/Duplicate.t.sol:A".to_string(),
                        1,
                        Address::from_str("0x5a443704dd4b594b382c22a083e2bd3090a6fef3").unwrap(),
                    ),
                    (
                        "duplicate/Duplicate.t.sol:C".to_string(),
                        3,
                        Address::from_str("0x8be503bcded90ed42eff31f56199399b2b0154ca").unwrap(),
                    ),
                    (
                        "duplicate/Duplicate.t.sol:E".to_string(),
                        5,
                        Address::from_str("0x29b2440db4a256b0c1e6d3b4cdcaa68e2440a08f").unwrap(),
                    ),
                ],
            )
            .assert_dependencies(
                "duplicate/Duplicate.t.sol:DuplicateLibraryLinkingTest".to_string(),
                vec![
                    (
                        "duplicate/Duplicate.t.sol:A".to_string(),
                        1,
                        Address::from_str("0x5a443704dd4b594b382c22a083e2bd3090a6fef3").unwrap(),
                    ),
                    (
                        "duplicate/Duplicate.t.sol:B".to_string(),
                        2,
                        Address::from_str("0x47e9fbef8c83a1714f1951f142132e6e90f5fa5d").unwrap(),
                    ),
                    (
                        "duplicate/Duplicate.t.sol:A".to_string(),
                        1,
                        Address::from_str("0x5a443704dd4b594b382c22a083e2bd3090a6fef3").unwrap(),
                    ),
                    (
                        "duplicate/Duplicate.t.sol:C".to_string(),
                        3,
                        Address::from_str("0x8be503bcded90ed42eff31f56199399b2b0154ca").unwrap(),
                    ),
                    (
                        "duplicate/Duplicate.t.sol:B".to_string(),
                        2,
                        Address::from_str("0x47e9fbef8c83a1714f1951f142132e6e90f5fa5d").unwrap(),
                    ),
                    (
                        "duplicate/Duplicate.t.sol:D".to_string(),
                        4,
                        Address::from_str("0x47c5e40890bce4a473a49d7501808b9633f29782").unwrap(),
                    ),
                    (
                        "duplicate/Duplicate.t.sol:A".to_string(),
                        1,
                        Address::from_str("0x5a443704dd4b594b382c22a083e2bd3090a6fef3").unwrap(),
                    ),
                    (
                        "duplicate/Duplicate.t.sol:C".to_string(),
                        3,
                        Address::from_str("0x8be503bcded90ed42eff31f56199399b2b0154ca").unwrap(),
                    ),
                    (
                        "duplicate/Duplicate.t.sol:E".to_string(),
                        5,
                        Address::from_str("0x29b2440db4a256b0c1e6d3b4cdcaa68e2440a08f").unwrap(),
                    ),
                ],
            )
            .test_with_sender_and_nonce(Address::default(), 1);
    }
}
