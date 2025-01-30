//! forge install and update tests

use forge::{DepIdentifier, Lockfile};
use foundry_cli::utils::Submodules;
use foundry_compilers::artifacts::Remapping;
use foundry_config::Config;
use foundry_test_utils::util::{pretty_err, read_string, TestCommand};
use semver::Version;
use std::{fs, path::PathBuf, process::Command, str::FromStr};

// checks missing dependencies are auto installed
forgetest_init!(can_install_missing_deps_build, |prj, cmd| {
    prj.clear();

    // wipe forge-std
    let forge_std_dir = prj.root().join("lib/forge-std");
    pretty_err(&forge_std_dir, fs::remove_dir_all(&forge_std_dir));

    // Build the project
    cmd.arg("build").assert_success().stdout_eq(str![[r#"
Missing dependencies found. Installing now...

[UPDATING_DEPENDENCIES]
[COMPILING_FILES] with [SOLC_VERSION]
[SOLC_VERSION] [ELAPSED]
Compiler run successful!

"#]]);

    // Expect compilation to be skipped as no files have changed
    cmd.forge_fuse().arg("build").assert_success().stdout_eq(str![[r#"
No files changed, compilation skipped

"#]]);
});

// checks missing dependencies are auto installed
forgetest_init!(can_install_missing_deps_test, |prj, cmd| {
    prj.clear();

    // wipe forge-std
    let forge_std_dir = prj.root().join("lib/forge-std");
    pretty_err(&forge_std_dir, fs::remove_dir_all(&forge_std_dir));

    cmd.arg("test").assert_success().stdout_eq(str![[r#"
Missing dependencies found. Installing now...

[UPDATING_DEPENDENCIES]
[COMPILING_FILES] with [SOLC_VERSION]
[SOLC_VERSION] [ELAPSED]
Compiler run successful!

Ran 2 tests for test/Counter.t.sol:CounterTest
[PASS] testFuzz_SetNumber(uint256) (runs: 256, [AVG_GAS])
[PASS] test_Increment() ([GAS])
Suite result: ok. 2 passed; 0 failed; 0 skipped; [ELAPSED]

Ran 1 test suite [ELAPSED]: 2 tests passed, 0 failed, 0 skipped (2 total tests)

"#]]);
});

// test to check that install/remove works properly
forgetest!(can_install_and_remove, |prj, cmd| {
    cmd.git_init();

    let libs = prj.root().join("lib");
    let git_mod = prj.root().join(".git/modules/lib");
    let git_mod_file = prj.root().join(".gitmodules");

    let forge_std = libs.join("forge-std");
    let forge_std_mod = git_mod.join("forge-std");

    let install = |cmd: &mut TestCommand| {
        cmd.forge_fuse()
            .args(["install", "foundry-rs/forge-std", "--no-commit"])
            .assert_success()
            .stdout_eq(str![[r#"
Installing forge-std in [..] (url: Some("https://github.com/foundry-rs/forge-std"), tag: None)
    Installed forge-std[..]

"#]]);

        assert!(forge_std.exists());
        assert!(forge_std_mod.exists());

        let submods = read_string(&git_mod_file);
        assert!(submods.contains("https://github.com/foundry-rs/forge-std"));
    };

    let remove = |cmd: &mut TestCommand, target: &str| {
        // TODO: flaky behavior with URL, sometimes it is None, sometimes it is Some("https://github.com/lib/forge-std")
        cmd.forge_fuse().args(["remove", "--force", target]).assert_success().stdout_eq(str![[
            r#"
Removing 'forge-std' in [..], (url: [..], tag: None)

"#
        ]]);

        assert!(!forge_std.exists());
        assert!(!forge_std_mod.exists());
        let submods = read_string(&git_mod_file);
        assert!(!submods.contains("https://github.com/foundry-rs/forge-std"));
    };

    install(&mut cmd);
    remove(&mut cmd, "forge-std");

    // install again and remove via relative path
    install(&mut cmd);
    remove(&mut cmd, "lib/forge-std");
});

// test to check we can run `forge install` in an empty dir <https://github.com/foundry-rs/foundry/issues/6519>
forgetest!(can_install_empty, |prj, cmd| {
    // create
    cmd.git_init();
    cmd.forge_fuse().args(["install"]);
    cmd.assert_empty_stdout();

    // create initial commit
    fs::write(prj.root().join("README.md"), "Initial commit").unwrap();

    cmd.git_add();
    cmd.git_commit("Initial commit");

    cmd.forge_fuse().args(["install"]);
    cmd.assert_empty_stdout();
});

// test to check that package can be reinstalled after manually removing the directory
forgetest!(can_reinstall_after_manual_remove, |prj, cmd| {
    cmd.git_init();

    let libs = prj.root().join("lib");
    let git_mod = prj.root().join(".git/modules/lib");
    let git_mod_file = prj.root().join(".gitmodules");

    let forge_std = libs.join("forge-std");
    let forge_std_mod = git_mod.join("forge-std");

    let install = |cmd: &mut TestCommand| {
        cmd.forge_fuse()
            .args(["install", "foundry-rs/forge-std", "--no-commit"])
            .assert_success()
            .stdout_eq(str![[r#"
Installing forge-std in [..] (url: Some("https://github.com/foundry-rs/forge-std"), tag: None)
    Installed forge-std tag=[..]"#]]);

        assert!(forge_std.exists());
        assert!(forge_std_mod.exists());

        let submods = read_string(&git_mod_file);
        assert!(submods.contains("https://github.com/foundry-rs/forge-std"));
    };

    install(&mut cmd);
    fs::remove_dir_all(forge_std.clone()).expect("Failed to remove forge-std");

    // install again
    install(&mut cmd);
});

// test that we can repeatedly install the same dependency without changes
forgetest!(can_install_repeatedly, |_prj, cmd| {
    cmd.git_init();

    cmd.forge_fuse().args(["install", "foundry-rs/forge-std"]);
    for _ in 0..3 {
        cmd.assert_success();
    }
});

// test that by default we install the latest semver release tag
// <https://github.com/openzeppelin/openzeppelin-contracts>
forgetest!(can_install_latest_release_tag, |prj, cmd| {
    cmd.git_init();
    cmd.forge_fuse().args(["install", "openzeppelin/openzeppelin-contracts"]);
    cmd.assert_success();

    let dep = prj.paths().libraries[0].join("openzeppelin-contracts");
    assert!(dep.exists());

    // the latest release at the time this test was written
    let version: Version = "4.8.0".parse().unwrap();
    let out = Command::new("git").current_dir(&dep).args(["describe", "--tags"]).output().unwrap();
    let tag = String::from_utf8_lossy(&out.stdout);
    let current: Version = tag.as_ref().trim_start_matches('v').trim().parse().unwrap();

    assert!(current >= version);
});

forgetest!(can_update_and_retain_tag_revs, |prj, cmd| {
    cmd.git_init();

    // Installs oz at release tag
    cmd.forge_fuse()
        .args(["install", "openzeppelin/openzeppelin-contracts@v5.1.0"])
        .assert_success();

    // Install solady pinned to rev i.e https://github.com/Vectorized/solady/commit/513f581675374706dbe947284d6b12d19ce35a2a
    cmd.forge_fuse().args(["install", "vectorized/solady@513f581"]).assert_success();

    let out = cmd.git_submodule_status();
    let status = String::from_utf8_lossy(&out.stdout);
    let mut lockfile_init = Lockfile::new(prj.root());

    lockfile_init.read().unwrap();

    let deps = lockfile_init.iter().map(|(path, dep_id)| (path, dep_id)).collect::<Vec<_>>();
    assert_eq!(deps.len(), 2);
    assert_eq!(
        deps[0],
        (
            &PathBuf::from("lib/openzeppelin-contracts"),
            &DepIdentifier::Tag {
                name: "v5.1.0".to_string(),
                rev: "69c8def5f222ff96f2b5beff05dfba996368aa79".to_string(),
                r#override: false
            }
        )
    );

    assert_eq!(
        deps[1],
        (
            &PathBuf::from("lib/solady"),
            &DepIdentifier::Rev { rev: "513f581".to_string(), r#override: false }
        )
    );

    let submodules_init: Submodules = status.parse().unwrap();

    cmd.forge_fuse().arg("update").assert_success();

    let out = cmd.git_submodule_status();
    let status = String::from_utf8_lossy(&out.stdout);
    let submodules_update: Submodules = status.parse().unwrap();
    assert_eq!(submodules_init, submodules_update);

    let mut lockfile_update = Lockfile::new(prj.root());

    lockfile_update.read().unwrap();

    let deps = lockfile_update.iter().map(|(path, dep_id)| (path, dep_id)).collect::<Vec<_>>();

    assert_eq!(deps.len(), 2);
    assert_eq!(
        deps[1],
        (
            &PathBuf::from("lib/openzeppelin-contracts"),
            &DepIdentifier::Tag {
                name: "v5.1.0".to_string(),
                rev: "69c8def5f222ff96f2b5beff05dfba996368aa79".to_string(),
                r#override: false
            }
        )
    );
    assert_eq!(
        deps[0],
        (
            &PathBuf::from("lib/solady"),
            &DepIdentifier::Rev { rev: "513f581".to_string(), r#override: false }
        )
    );
});

forgetest!(can_override_tag_in_update, |_prj, cmd| {
    cmd.git_init();

    // Installs oz at release tag
    cmd.forge_fuse()
        .args(["install", "openzeppelin/openzeppelin-contracts@v5.0.2"])
        .assert_success();

    cmd.forge_fuse().args(["install", "vectorized/solady@513f581"]).assert_success();

    let out = cmd.git_submodule_status();
    let status = String::from_utf8_lossy(&out.stdout);

    let submodules_init: Submodules = status.parse().unwrap();

    // Update oz to a different release tag
    cmd.forge_fuse()
        .args(["update", "openzeppelin/openzeppelin-contracts@v5.1.0"])
        .assert_success();

    let out = cmd.git_submodule_status();
    let status = String::from_utf8_lossy(&out.stdout);

    let submodules_update: Submodules = status.parse().unwrap();

    assert_ne!(submodules_init.0[0], submodules_update.0[0]);
    assert_eq!(submodules_init.0[1], submodules_update.0[1]);
});

// Ref: https://github.com/foundry-rs/foundry/pull/9522#pullrequestreview-2494431518
forgetest!(should_not_update_tagged_deps, |prj, cmd| {
    cmd.git_init();

    // Installs oz at release tag
    cmd.forge_fuse()
        .args(["install", "openzeppelin/openzeppelin-contracts@tag=v4.9.4"])
        .assert_success();

    let out = cmd.git_submodule_status();
    let status = String::from_utf8_lossy(&out.stdout);
    let submodules_init: Submodules = status.parse().unwrap();

    cmd.forge_fuse().arg("update").assert_success();

    let out = cmd.git_submodule_status();
    let status = String::from_utf8_lossy(&out.stdout);
    let submodules_update: Submodules = status.parse().unwrap();

    assert_eq!(submodules_init, submodules_update);

    // Check that halmos-cheatcodes dep is not added to oz deps
    let halmos_path = prj.paths().libraries[0].join("openzeppelin-contracts/lib/halmos-cheatcodes");

    assert!(!halmos_path.exists());
});

forgetest!(can_remove_dep_from_foundry_lock, |prj, cmd| {
    cmd.git_init();

    cmd.forge_fuse()
        .args(["install", "openzeppelin/openzeppelin-contracts@tag=v4.9.4"])
        .assert_success();

    cmd.forge_fuse().args(["install", "vectorized/solady@513f581"]).assert_success();
    cmd.forge_fuse().args(["remove", "openzeppelin-contracts"]).assert_success();

    let mut lock = Lockfile::new(prj.root());

    lock.read().unwrap();

    assert!(lock.get(&PathBuf::from("lib/openzeppelin-contracts")).is_none());
});

forgetest!(
    #[cfg_attr(windows, ignore = "weird git fail")]
    can_sync_foundry_lock,
    |prj, cmd| {
        cmd.git_init();

        cmd.forge_fuse().args(["install", "foundry-rs/forge-std@master"]).assert_success();

        cmd.forge_fuse().args(["install", "vectorized/solady"]).assert_success();

        fs::remove_file(prj.root().join("foundry.lock")).unwrap();

        // sync submodules and write foundry.lock
        cmd.forge_fuse().arg("install").assert_success();

        let mut lock = forge::Lockfile::new(prj.root());
        lock.read().unwrap();

        assert!(matches!(
            lock.get(&PathBuf::from("lib/forge-std")).unwrap(),
            &DepIdentifier::Rev { .. }
        ));
        assert!(matches!(
            lock.get(&PathBuf::from("lib/solady")).unwrap(),
            &DepIdentifier::Tag { .. }
        ));
    }
);

// Tests that forge update doesn't break a working dependency by recursively updating nested
// dependencies
forgetest!(
    #[cfg_attr(windows, ignore = "weird git fail")]
    can_update_library_with_outdated_nested_dependency,
    |prj, cmd| {
        cmd.git_init();

        let libs = prj.root().join("lib");
        let git_mod = prj.root().join(".git/modules/lib");
        let git_mod_file = prj.root().join(".gitmodules");

        // get paths to check inside install fn
        let package = libs.join("forge-5980-test");
        let package_mod = git_mod.join("forge-5980-test");

        // install main dependency
        cmd.forge_fuse()
            .args(["install", "evalir/forge-5980-test", "--no-commit"])
            .assert_success()
            .stdout_eq(str![[r#"
Installing forge-5980-test in [..] (url: Some("https://github.com/evalir/forge-5980-test"), tag: None)
    Installed forge-5980-test

"#]]);

        // assert paths exist
        assert!(package.exists());
        assert!(package_mod.exists());

        let submods = read_string(git_mod_file);
        assert!(submods.contains("https://github.com/evalir/forge-5980-test"));

        // try to update the top-level dependency; there should be no update for this dependency,
        // but its sub-dependency has upstream (breaking) changes; forge should not attempt to
        // update the sub-dependency
        cmd.forge_fuse().args(["update", "lib/forge-5980-test"]).assert_empty_stdout();

        // add explicit remappings for test file
        let config = Config {
            remappings: vec![
                Remapping::from_str("forge-5980-test/=lib/forge-5980-test/src/").unwrap().into(),
                // explicit remapping for sub-dependendy seems necessary for some reason
                Remapping::from_str(
                    "forge-5980-test-dep/=lib/forge-5980-test/lib/forge-5980-test-dep/src/",
                )
                .unwrap()
                .into(),
            ],
            ..Default::default()
        };
        prj.write_config(config);

        // create test file that uses the top-level dependency; if the sub-dependency is updated,
        // compilation will fail
        prj.add_source(
            "CounterCopy",
            r#"
import "forge-5980-test/Counter.sol";
contract CounterCopy is Counter {
}
   "#,
        )
        .unwrap();

        // build and check output
        cmd.forge_fuse().arg("build").assert_success().stdout_eq(str![[r#"
[COMPILING_FILES] with [SOLC_VERSION]
[SOLC_VERSION] [ELAPSED]
Compiler run successful!

"#]]);
    }
);
