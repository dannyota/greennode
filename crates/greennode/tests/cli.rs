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
        &["--read-only=private-canary", "version"],
    ] {
        let output = greennode(args);
        assert_eq!(output.status.code(), Some(2));
        assert!(output.stdout.is_empty());
        let doc: serde_json::Value =
            serde_json::from_slice(&output.stderr).expect("one JSON error");
        assert_eq!(doc["error"]["code"], "InvalidUsage");
        assert!(
            doc["error"]["message"]
                .as_str()
                .is_some_and(|s| !s.is_empty())
        );
        assert_eq!(output.stderr.iter().filter(|&&c| c == b'\n').count(), 1);
        let stderr = String::from_utf8(output.stderr).unwrap();
        for secret in ["no-such-command", "--nope", "extra", "private-canary"] {
            assert!(!stderr.contains(secret));
        }
        assert!(stderr.starts_with("{\"error\":{\"code\":\"InvalidUsage\",\"message\":"));
    }
}

#[test]
fn clustered_exporter_help_stays_hidden() {
    let output = greennode(&["__command-inventory", "-hh"]);
    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
    assert!(!String::from_utf8_lossy(&output.stderr).contains("__command-inventory"));
}

#[test]
fn clustered_help_does_not_bypass_validation_or_reveal_exporter() {
    for args in [&["-vh", "--nope"][..], &["__command-inventory", "-hh"]] {
        let output = greennode(args);
        assert_eq!(output.status.code(), Some(2));
        assert!(output.stdout.is_empty());
        assert!(!String::from_utf8_lossy(&output.stderr).contains("__command-inventory"));
    }
    assert!(greennode(&["-h"]).status.success());
}

#[test]
fn hidden_inventory_has_no_public_help() {
    for args in [
        &["help", "__command-inventory"][..],
        &["__command-inventory", "--help"],
    ] {
        let output = greennode(args);
        assert!(output.stdout.is_empty());
        assert_eq!(output.status.code(), Some(2));
        assert!(!String::from_utf8_lossy(&output.stderr).contains("__command-inventory"));
    }
    let output = greennode(&["--help"]);
    assert!(output.status.success());
    assert!(!String::from_utf8_lossy(&output.stdout).contains("__command-inventory"));
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
fn version_help_includes_full_path_and_globals() {
    for args in [&["version", "-h"][..], &["help", "version"]] {
        let output = greennode(args);
        assert!(output.status.success());
        assert!(output.stderr.is_empty());
        let text = String::from_utf8(output.stdout).unwrap();
        assert!(text.contains("Usage: greennode version"), "{text}");
        assert!(text.contains("--profile"), "{text}");
    }
}

#[test]
fn no_arguments_print_help() {
    let output = greennode(&[]);
    assert!(output.status.success());
    assert!(String::from_utf8_lossy(&output.stdout).contains("Command-line access to GreenNode"));
    assert!(output.stderr.is_empty());
}

#[test]
fn boolean_global_flags_accept_optional_equals_values() {
    for flag in ["yes", "read-only", "debug"] {
        for value in ["", "=true", "=false"] {
            let arg = format!("--{flag}{value}");
            for args in [vec![arg.as_str(), "version"], vec!["version", arg.as_str()]] {
                assert!(greennode(&args).status.success(), "{args:?}");
            }
        }
        for args in [
            vec![format!("--{flag}=invalid"), "version".into()],
            vec!["version".into(), format!("--{flag}=invalid")],
            vec![format!("--{flag}"), "false".into(), "version".into()],
            vec!["version".into(), format!("--{flag}"), "false".into()],
        ] {
            let refs: Vec<&str> = args.iter().map(String::as_str).collect();
            let output = greennode(&refs);
            assert_eq!(output.status.code(), Some(2));
            assert!(output.stdout.is_empty());
        }
    }
}

#[test]
fn unknown_flags_are_rejected_even_with_help() {
    for args in [
        &["--help", "--nope"][..],
        &["--nope", "--help"],
        &["version", "--help", "----nope"],
        &["version", "--nope", "--help"],
    ] {
        let output = greennode(args);
        assert_eq!(output.status.code(), Some(2));
        assert!(output.stdout.is_empty());
    }
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

#[test]
fn billing_missing_budget_uuid_is_usage_error() {
    let output = greennode(&["billing", "get-budget"]);
    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
    assert_eq!(
        output.stderr,
        b"{\"error\":{\"code\":\"InvalidUsage\",\"message\":\"--budget-uuid is required\"}}\n"
    );
}

#[test]
fn billing_missing_credentials_exits_three() {
    let output = greennode(&["billing", "list-budgets", "--region", "hcm-3"]);
    assert_eq!(output.status.code(), Some(3));
    assert!(output.stdout.is_empty());
    let error: serde_json::Value = serde_json::from_slice(&output.stderr).unwrap();
    assert_eq!(error["error"]["code"], "NoCredentials");
}
