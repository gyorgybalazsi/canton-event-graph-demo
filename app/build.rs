//! Generates the Rust types for the ticket purchase model at build time, by
//! running daml-rust-codegen on the model's DAR. The output goes to OUT_DIR and
//! `src/model.rs` includes it, so the types always match the DAR.
//!
//! - `DAML_RUST_CODEGEN`: the codegen program (default: `.demo/bin/daml-rust-codegen`,
//!   where `just tools` installs it, then `daml-rust-codegen` on the PATH)
//! - `TICKET_PURCHASE_DAR`: the DAR (default: `daml/.daml/dist/ticket-purchase-0.1.0.dar`,
//!   where `just build-dar` puts it)
//! - `TICKET_PURCHASE_GENERATED`: an already generated file to use instead of
//!   running the codegen (for builds without the Daml SDK)

use std::env;
use std::path::{Path, PathBuf};
use std::process::Command;

fn main() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    let out = PathBuf::from(env::var("OUT_DIR").unwrap()).join("ticket_purchase.rs");

    println!("cargo::rerun-if-env-changed=DAML_RUST_CODEGEN");
    println!("cargo::rerun-if-env-changed=TICKET_PURCHASE_DAR");
    println!("cargo::rerun-if-env-changed=TICKET_PURCHASE_GENERATED");

    if let Ok(generated) = env::var("TICKET_PURCHASE_GENERATED") {
        println!("cargo::rerun-if-changed={generated}");
        std::fs::copy(&generated, &out)
            .unwrap_or_else(|e| panic!("could not copy {generated}: {e}"));
        return;
    }

    let dar = env::var("TICKET_PURCHASE_DAR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| root.join("daml/.daml/dist/ticket-purchase-0.1.0.dar"));
    println!("cargo::rerun-if-changed={}", dar.display());
    if !dar.exists() {
        panic!(
            "\n\nThe ticket purchase DAR is missing: {}\nBuild it first with `just build-dar` \
             (needs the Daml SDK), or set TICKET_PURCHASE_DAR.\n",
            dar.display()
        );
    }

    let codegen = env::var("DAML_RUST_CODEGEN")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            let local = root.join(".demo/bin/daml-rust-codegen");
            if local.exists() {
                local
            } else {
                PathBuf::from("daml-rust-codegen")
            }
        });
    println!("cargo::rerun-if-changed={}", codegen.display());

    let status = Command::new(&codegen)
        .arg("--dar")
        .arg(&dar)
        .arg("-o")
        .arg(&out)
        .status()
        .unwrap_or_else(|e| {
            panic!(
                "\n\nCould not run {}: {e}\nInstall it with `just tools`, or set DAML_RUST_CODEGEN.\n",
                codegen.display()
            )
        });
    if !status.success() {
        panic!(
            "{} failed on {}: {status}",
            codegen.display(),
            dar.display()
        );
    }
}
