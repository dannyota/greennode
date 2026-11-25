use std::process::{Command, Output};

fn greennode(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_greennode"))
        .args(args)
        .env_clear()
        .output()
        .expect("the greennode binary runs")
}

#[test]
fn version_command_prints_the_crate_version() {
    let output = greennode(&["version"]);
    assert!(output.status.success());
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        format!("greennode {}\n", env!("CARGO_PKG_VERSION"))
    );
}

#[test]
fn unknown_command_exits_2_and_keeps_stdout_empty() {
    let output = greennode(&["no-such-command"]);
    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
}
