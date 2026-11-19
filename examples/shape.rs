//! Print the shape of a member-site answer read on stdin, for docs/api.md
//! and the synthetic fixtures. Not part of `caremark`.
//!
//! ```sh
//! caremark api POST financial-summary --profile adult1 --data '{"data":{...}}' \
//!   | cargo run -q --example shape
//! ```
//!
//! It prints keys and types only, never a value (`caremark::shape`): each
//! path once, with the kind of string it holds (a date, a number, a word,
//! text). A key holding one of the household's names, in any case, prints
//! as `<key>`. The names are gathered in memory and never printed (counts
//! only, on stderr): from the answer's own name fields
//! (`Redactor::learn_names`), from this Mac account's full name (`id -F`),
//! and from `~/.config/caremark/redact-words` when the owner wrote one (one
//! per line, mode 0600; a file others can read stops it, exit 2).

use std::io::Read;

use caremark::shape::Redactor;
use pk_cli_core::CliError;

fn main() {
    if let Err(e) = run() {
        eprintln!("error: {}", e.message());
        std::process::exit(e.exit_code());
    }
}

fn run() -> Result<(), CliError> {
    let path = Redactor::default_path().ok_or_else(|| CliError::Usage("no home directory".into()))?;
    let mut redact = Redactor::from_file_if_present(&path)?;
    let from_file = redact.len();
    redact.add_names(&account_name());
    let mut text = String::new();
    std::io::stdin()
        .read_to_string(&mut text)
        .map_err(|e| CliError::Usage(format!("reading stdin: {e}")))?;
    let doc: serde_json::Value =
        serde_json::from_str(&text).map_err(|_| CliError::Usage("stdin is not one JSON document".into()))?;
    let learned = redact.learn_names(&doc);
    eprintln!(
        "{from_file} deny word(s) from {}, plus this account's name and {learned} name(s) from the answer",
        path.display()
    );
    for line in redact.shape_lines(&doc) {
        println!("{line}");
    }
    Ok(())
}

/// This Mac account's full name, which a household's answers may carry.
fn account_name() -> String {
    std::process::Command::new("id")
        .arg("-F")
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
        .unwrap_or_default()
}
