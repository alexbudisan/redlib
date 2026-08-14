use std::process::{Command, ExitStatus, Output};
use std::time::{SystemTime, UNIX_EPOCH};

#[cfg(not(target_os = "windows"))]
use std::os::unix::process::ExitStatusExt;

#[cfg(target_os = "windows")]
use std::os::windows::process::ExitStatusExt;

fn main() {
	println!("cargo:rerun-if-changed=src/");
	println!("cargo:rerun-if-env-changed=REDLIB_BUILD_ID");
	let output = String::from_utf8(
		Command::new("git")
			.args(["rev-parse", "HEAD"])
			.output()
			.unwrap_or(Output {
				stdout: vec![],
				stderr: vec![],
				status: ExitStatus::from_raw(0),
			})
			.stdout,
	)
	.unwrap_or_default();
	let git_hash = if output == String::default() { "dev".into() } else { output.trim().to_owned() };
	println!("cargo:rustc-env=GIT_HASH={git_hash}");

	let configured_build_id = std::env::var("REDLIB_BUILD_ID").unwrap_or_else(|_| "auto".to_owned());
	let build_id = if configured_build_id != "auto" && !configured_build_id.trim().is_empty() {
		configured_build_id
	} else {
		let timestamp = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs();
		let short_hash = &git_hash[..git_hash.len().min(12)];
		format!("local-{short_hash}-{timestamp}")
	};
	println!("cargo:rustc-env=REDLIB_BUILD_ID={build_id}");
}
