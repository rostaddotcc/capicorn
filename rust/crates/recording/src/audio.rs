//! Audio input discovery through `pactl` (PipeWire's PulseAudio front end).
//!
//! Recording audio is done by ffmpeg (`-f pulse -i <source>`), so this module
//! only needs to answer "which source names exist, and what do they mean to a
//! human". Every failure degrades to an empty list: a headless box, a missing
//! `pactl` or a dead sound server is a normal state for a settings screen, not
//! an error worth failing a recording start over.

use std::process::{Command, Stdio};

use crate::AudioSource;

/// Every capture source this machine offers.
pub(crate) fn list() -> Vec<AudioSource> {
    let output = Command::new("pactl")
        .args(["list", "short", "sources"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output();

    match output {
        Ok(output) if output.status.success() => {
            parse_pactl_sources(&String::from_utf8_lossy(&output.stdout))
        }
        _ => Vec::new(),
    }
}

/// Parse `pactl list short sources`: one `index\tname\tdriver\tspec\tstate` row
/// per device.
///
/// The source *name* is the id (that is what `-f pulse -i` wants). Anything that
/// is not such a row — a header, a blank line, output from a renamed tool — is
/// skipped, so unexpected output can never invent a device.
pub(crate) fn parse_pactl_sources(text: &str) -> Vec<AudioSource> {
    let mut out = Vec::new();
    for line in text.lines() {
        let mut fields = line.split('\t');
        let index = fields.next().unwrap_or("").trim();
        // A real row always starts with the numeric index; this also rejects the
        // column header of `pactl list sources`.
        if index.parse::<u32>().is_err() {
            continue;
        }
        let Some(name) = fields.next().map(str::trim) else {
            continue;
        };
        if name.is_empty() || name.starts_with('@') {
            continue;
        }
        out.push(AudioSource {
            id: name.to_string(),
            name: humanise_source_name(name),
            kind: source_kind(name).to_string(),
        });
    }
    out
}

/// Which broad class a source belongs to, so a picker can group microphones and
/// system-audio monitors apart.
pub(crate) fn source_kind(id: &str) -> &'static str {
    if id.ends_with(".monitor") {
        "monitor"
    } else if id.starts_with("alsa_input")
        || id.contains("input")
        || id.contains("mic")
        || id.starts_with("virmidi")
    {
        "input"
    } else {
        "other"
    }
}

/// Turn a PulseAudio id into a label a person recognises.
///
/// `alsa_output.pci-0000_00_1f.3.analog-stereo.monitor` becomes
/// "Monitor of Analog stereo": ids are driver-prefixed, dotted and hyphenated,
/// and the last segment is the part with meaning.
pub(crate) fn humanise_source_name(raw: &str) -> String {
    let (base, monitor) = match raw.strip_suffix(".monitor") {
        Some(base) => (base, true),
        None => (raw, false),
    };

    let mut segments = base.split('.').filter(|s| !s.is_empty());
    let first = segments.next().unwrap_or(base);
    let last = segments.next_back().unwrap_or(first);

    let pretty = last.replace(['-', '_'], " ");
    let pretty = pretty.trim();
    let pretty = if pretty.is_empty() {
        "Audio device"
    } else {
        pretty
    };
    let capitalised = capitalise(pretty);

    if monitor {
        format!("Monitor of {capitalised}")
    } else {
        capitalised
    }
}

fn capitalise(text: &str) -> String {
    let mut chars = text.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PACTL_OUTPUT: &str = "\
53\talsa_output.pci-0000_00_1f.3.analog-stereo.monitor\tPipeWire\ts32le 2ch 48000Hz\tIDLE
54\talsa_input.pci-0000_00_1f.3.analog-stereo\tPipeWire\ts32le 2ch 48000Hz\tSUSPENDED
55\t@DEFAULT_SOURCE@\tPipeWire\ts32le 2ch 48000Hz\tIDLE
";

    #[test]
    fn pactl_rows_are_parsed_and_named_for_a_human() {
        let sources = parse_pactl_sources(PACTL_OUTPUT);
        assert_eq!(sources.len(), 2, "the @DEFAULT_SOURCE@ alias is skipped");
        assert_eq!(
            sources[0].id,
            "alsa_output.pci-0000_00_1f.3.analog-stereo.monitor"
        );
        assert_eq!(sources[0].name, "Monitor of Analog stereo");
        assert_eq!(sources[0].kind, "monitor");
        assert_eq!(sources[1].id, "alsa_input.pci-0000_00_1f.3.analog-stereo");
        assert_eq!(sources[1].name, "Analog stereo");
        assert_eq!(sources[1].kind, "input");
    }

    #[test]
    fn garbage_pactl_output_yields_no_devices() {
        assert!(parse_pactl_sources("").is_empty());
        assert!(parse_pactl_sources("Source #0\n\tState: RUNNING\n").is_empty());
        assert!(parse_pactl_sources("nonsense\n1\t\n").is_empty());
        assert!(parse_pactl_sources("not-a-number\talsa_input.foo\n").is_empty());
    }

    #[test]
    fn an_awkward_id_still_gets_a_readable_label() {
        assert_eq!(
            humanise_source_name("alsa_input.usb-046d_HD_Pro"),
            "Usb 046d HD Pro"
        );
        assert_eq!(humanise_source_name("some-device"), "Some device");
        // No dots at all: the whole id is the label.
        assert_eq!(humanise_source_name("mic"), "Mic");
    }

    #[test]
    fn unknown_id_shapes_are_classified_as_other() {
        assert_eq!(source_kind("weird.thing"), "other");
        assert_eq!(source_kind("alsa_output.x.monitor"), "monitor");
        assert_eq!(source_kind("alsa_input.usb-mic"), "input");
    }

    #[test]
    fn listing_audio_sources_is_graceful_on_this_host() {
        // Whatever the host state (headless, no pactl, no server), this returns.
        let sources = list();
        for source in &sources {
            assert!(!source.id.is_empty());
            assert!(source.id != "@DEFAULT_SOURCE@" && source.id != "@DEFAULT_MONITOR@");
        }
    }
}
