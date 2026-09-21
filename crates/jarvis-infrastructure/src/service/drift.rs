//! Detecting a service definition that points at the wrong executable.
//!
//! `ACC-003` seeds a service-path fault: the unit, plist, or task that starts the
//! daemon names a path that is no longer the installed one. After an update that
//! moved the version directory, or a partial uninstall, the registered service
//! keeps starting an old or missing binary — the service exists, reports
//! `installed`, and serves nothing.
//!
//! ## Parsing the installed definition, not the intended one
//!
//! The detection has to read what is **on disk**, because the failure mode is
//! exactly a definition that disagrees with what this build would write. Comparing
//! `spec.executable` to itself would always agree and prove nothing.
//!
//! The parsers here are pure functions over definition text, so they are asserted
//! on any host without installing a service. That matters more than usual here:
//! the platform this defect is most likely on is the platform that cannot be
//! exercised from the authoring host.
//!
//! ## Scope, stated rather than implied
//!
//! - **systemd** and **launchd** write a definition file, so drift is detected by
//!   reading it.
//! - **Windows** defines its task entirely by command line and exposes it only
//!   through `schtasks /query /xml`, so there is no file to read with the current
//!   dependency set. Detection there is a **named gap**, and the check says so by
//!   reporting nothing rather than reporting a false "no drift".

use std::path::PathBuf;

/// The executable named by a systemd unit definition, if it can be read.
///
/// systemd allows one `ExecStart=` line per service section, and the value is
/// either a bare path or a quoted string. The quoting is undone here with the
/// inverse of [`crate::service::systemd_escape`]: a backslash escapes the next
/// character, and a double quote delimits.
///
/// Only the **first** word of the value is taken, because `ExecStart=` may carry
/// arguments after the executable.
#[must_use]
pub fn systemd_executable(definition: &str) -> Option<PathBuf> {
    let value = definition
        .lines()
        .find_map(|line| line.strip_prefix("ExecStart="))?;
    first_word(value.trim()).map(PathBuf::from)
}

/// Returns the executable named by a launchd plist, if it can be read.
///
/// launchd gives the program as the first entry of the `ProgramArguments` array.
/// The search is positional rather than a full XML parse: the plist is generated
/// by this crate with a known shape, and pulling in an XML dependency to re-read a
/// file it wrote would be a dependency for a format this code already controls.
///
/// The keys are searched in order, so a `<string>` that belongs to another key
/// cannot be mistaken for the program.
#[must_use]
pub fn launchd_executable(definition: &str) -> Option<PathBuf> {
    let after_key = definition.split_once("<key>ProgramArguments</key>")?.1;
    let after_array = after_key.split_once("<array>")?.1;
    let value = after_array.split_once("<string>")?.1;
    let value = value.split_once("</string>")?.0;
    Some(PathBuf::from(xml_unescape(value)))
}

/// Reverses the escaping this crate applies to XML text nodes.
fn xml_unescape(value: &str) -> String {
    value
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        // `&amp;` is replaced last, or an escaped entity in the input would be
        // double-decoded: `&amp;lt;` means the literal text `&lt;`, not `<`.
        .replace("&amp;", "&")
}

/// Returns the first whitespace-separated word of `value`, undoing systemd quoting.
///
/// A quoted value is read to its closing quote so a path containing a space stays
/// one word, which is exactly why the escape function quotes it in the first place.
fn first_word(value: &str) -> Option<String> {
    if let Some(rest) = value.strip_prefix('"') {
        let mut out = String::new();
        let mut escaped = false;
        for character in rest.chars() {
            if escaped {
                out.push(character);
                escaped = false;
            } else if character == '\\' {
                escaped = true;
            } else if character == '"' {
                return Some(out);
            } else {
                out.push(character);
            }
        }
        // An unterminated quote is a malformed definition, not a path.
        return None;
    }
    let word = value.split_whitespace().next()?;
    if word.is_empty() {
        None
    } else {
        Some(word.to_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::{launchd_executable, systemd_executable};
    use std::path::PathBuf;

    #[test]
    fn a_systemd_unit_yields_its_executable() {
        let definition = "[Unit]\n\
             Description=JARVIS local daemon (jarvisd)\n\
             \n\
             [Service]\n\
             Type=simple\n\
             ExecStart=/opt/jarvis/versions/v0.2.0/bin/jarvisd\n\
             Restart=on-failure\n";
        assert_eq!(
            systemd_executable(definition),
            Some(PathBuf::from("/opt/jarvis/versions/v0.2.0/bin/jarvisd")),
        );
    }

    #[test]
    fn a_systemd_unit_with_arguments_yields_only_the_executable() {
        // `ExecStart=` may carry arguments, so taking the whole value would compare
        // a path plus flags against a path and report drift where there is none.
        let definition = "[Service]\nExecStart=/opt/jarvis/bin/jarvisd --foreground\n";
        assert_eq!(
            systemd_executable(definition),
            Some(PathBuf::from("/opt/jarvis/bin/jarvisd")),
        );
    }

    #[test]
    fn a_quoted_systemd_path_with_a_space_stays_one_word() {
        let definition = "[Service]\nExecStart=\"/opt/jarvis home/bin/jarvisd\" --foreground\n";
        assert_eq!(
            systemd_executable(definition),
            Some(PathBuf::from("/opt/jarvis home/bin/jarvisd")),
        );
    }

    #[test]
    fn a_quoted_systemd_path_with_an_escaped_quote_round_trips() {
        // The inverse of `systemd_escape`, which escapes `\` and `"`. Without the
        // escape handling the closing quote is found in the wrong place.
        let definition = "[Service]\nExecStart=\"/opt/a\\\"b/jarvisd\"\n";
        assert_eq!(
            systemd_executable(definition),
            Some(PathBuf::from("/opt/a\"b/jarvisd")),
        );
    }

    #[test]
    fn a_definition_without_an_exec_line_yields_nothing() {
        // The description mentions `jarvisd`; a search for the *name* would find it
        // and report a path that is not the executable.
        assert_eq!(
            systemd_executable("[Unit]\nDescription=JARVIS local daemon (jarvisd)\n"),
            None,
        );
        assert_eq!(systemd_executable(""), None);
        assert_eq!(
            systemd_executable("[Service]\nExecStart=\"unterminated\n"),
            None
        );
    }

    #[test]
    fn the_description_is_never_mistaken_for_the_executable() {
        // The trap that made the journey's service assertion pass on Windows for
        // the wrong reason, in the parser instead of the assertion.
        let definition = "[Unit]\n\
             Description=\"JARVIS local daemon (jarvisd)\"\n\
             [Service]\n\
             ExecStart=/opt/jarvis/bin/jarvis\n";
        assert_eq!(
            systemd_executable(definition),
            Some(PathBuf::from("/opt/jarvis/bin/jarvis")),
            "the parsed path must come from ExecStart, not the description",
        );
    }

    #[test]
    fn a_plist_yields_the_program_and_not_another_string() {
        let definition = "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
             <plist version=\"1.0\">\n\
             <dict>\n\
             \x20   <key>Label</key>\n\
             \x20   <string>com.jarvis.jarvisd</string>\n\
             \x20   <key>ProgramArguments</key>\n\
             \x20   <array>\n\
             \x20       <string>/Users/alice/install/bin/jarvisd</string>\n\
             \x20       <string>--foreground</string>\n\
             \x20   </array>\n\
             </dict>\n\
             </plist>\n";
        assert_eq!(
            launchd_executable(definition),
            Some(PathBuf::from("/Users/alice/install/bin/jarvisd")),
            "the Label string must not be mistaken for the program",
        );
    }

    #[test]
    fn a_plist_escaped_path_is_decoded() {
        // The inverse of the plist escaping this crate applies, and `&amp;` is
        // decoded last so an escaped entity is not double-decoded.
        let definition = "<key>ProgramArguments</key>\n\
             <array>\n\
             <string>/opt/a&amp;b/jarvisd</string>\n\
             </array>\n";
        assert_eq!(
            launchd_executable(definition),
            Some(PathBuf::from("/opt/a&b/jarvisd")),
        );

        let literal = "<key>ProgramArguments</key>\n\
             <array>\n\
             <string>/opt/a&amp;lt;b/jarvisd</string>\n\
             </array>\n";
        assert_eq!(
            launchd_executable(literal),
            Some(PathBuf::from("/opt/a&lt;b/jarvisd")),
        );
    }

    #[test]
    fn a_plist_without_a_program_block_yields_nothing() {
        assert_eq!(launchd_executable("<plist></plist>"), None);
        assert_eq!(
            launchd_executable("<key>ProgramArguments</key><array></array>"),
            None,
        );
    }

    /// The parser must agree with what the renderers actually write, or detection
    /// reports drift on a correct definition.
    ///
    /// The path is built from the host's temp directory rather than written as
    /// `/opt/...`, because an absolute path is a platform question: `/opt/...` is
    /// **not** absolute on Windows, so a hard-coded Unix path makes this test fail
    /// on the authoring host for a reason that has nothing to do with parsing. The
    /// space is kept, because quoting is what the round trip has to survive.
    #[test]
    fn the_parsers_read_back_what_the_renderers_write() {
        let executable = std::env::temp_dir().join("jarvis bin").join("jarvisd");
        let spec = crate::service::ServiceSpec::new(
            "jarvisd",
            executable.clone(),
            "JARVIS local daemon (jarvisd)",
        )
        .expect("spec");

        let unit = crate::service::systemd_unit_for_test(&spec);
        assert_eq!(
            systemd_executable(&unit),
            Some(executable.clone()),
            "{unit}"
        );

        let plist = crate::service::launchd_plist_for_test(&spec);
        assert_eq!(launchd_executable(&plist), Some(executable), "{plist}");
    }
}
