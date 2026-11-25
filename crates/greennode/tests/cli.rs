use std::process::{Command, Output};

fn greennode(args: &[&str]) -> Output {
    static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let home = std::env::temp_dir().join(format!(
        "greennode-cli-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&home).expect("create temporary home");
    let output = Command::new(env!("CARGO_BIN_EXE_greennode"))
        .args(args)
        .env_clear()
        .env("HOME", &home)
        .env("USERPROFILE", &home)
        .output()
        .expect("the greennode binary runs");
    std::fs::remove_dir_all(home).expect("remove temporary home");
    output
}

#[test]
fn version_command_prints_the_crate_version() {
    let os = match std::env::consts::OS {
        "macos" => "darwin",
        other => other,
    };
    let arch = match std::env::consts::ARCH {
        "x86_64" => "amd64",
        "aarch64" => "arm64",
        "x86" => "386",
        other => other,
    };
    for args in [&["version"][..], &["--version"], &["-v"]] {
        let output = greennode(args);
        assert!(output.status.success());
        assert_eq!(
            String::from_utf8_lossy(&output.stdout),
            format!("greennode {} {os}/{arch}\n", env!("CARGO_PKG_VERSION"))
        );
        assert!(output.stderr.is_empty());
    }
}

#[test]
fn unknown_command_exits_2_and_keeps_stdout_empty() {
    for args in [
        &["no-such-command"][..],
        &["version", "--nope"],
        &["version", "extra"],
    ] {
        let output = greennode(args);
        assert_eq!(output.status.code(), Some(2));
        assert!(output.stdout.is_empty());
        assert!(!output.stderr.is_empty());
    }
}

#[test]
fn shipped_commands_match_registry() {
    let output = greennode(&["__command-inventory"]);
    assert!(output.status.success());
    assert_eq!(
        output.stdout,
        include_bytes!("parity/shipped-commands.json")
    );
    assert!(output.stderr.is_empty());
}

#[test]
fn no_arguments_print_help() {
    let output = greennode(&[]);
    assert!(output.status.success());
    assert!(String::from_utf8_lossy(&output.stdout).contains("Command-line access to GreenNode"));
    assert!(output.stderr.is_empty());
}

#[test]
fn globals_work_on_both_sides_of_version() {
    for flag in [
        "--profile=x",
        "--region=x",
        "--project-id=x",
        "--output=json",
        "--query=x",
        "--yes",
        "--read-only",
        "--debug",
    ] {
        for args in [vec![flag, "version"], vec!["version", flag]] {
            assert!(greennode(&args).status.success(), "{args:?}");
        }
    }
}
