// The single definition of "this service preview names the daemon executable".
//
// This exists because the assertion has been wrong twice in the same way. It first
// keyed on `ExecStart=`, which is a systemd directive: it passed on Windows and
// Linux and failed on macOS, where launchd renders the path inside a `<string>`
// element. Then it lived as a literal regex duplicated in two files, so the guard
// that checks the matcher and the journey that uses it could drift apart without
// either failing.
//
// The matcher is therefore defined once and only once, and both files import it:
//
//   * `clean-machine-smoke.mjs` uses it on the real preview of the running
//     platform (this is the assertion that matters, and it runs on all five
//     tier-1 targets);
//   * `service-assertion-check.mjs` exercises it against the recorded rendering of
//     all three backends, which is how a platform-specific matcher is caught
//     without needing the platform.
//
// The match is on the executable **path**, not on a platform's key name, because
// every backend must name the path and none of them must agree on a directive
// name. Matching a path segment also avoids the trap that made the first version
// pass on Windows for the wrong reason: the unit *description* contains the word
// "jarvisd" inside quotes, so a bare-name check can be satisfied by text that is
// not the executable.

/** Matches `/path/to/jarvisd` or `\path\to\jarvisd.exe` as a path segment. */
export const DAEMON_ASSERTION = /[\\/]jarvisd(\.exe)?\b/;

/**
 * Returns whether `text` names the daemon executable.
 *
 * @param {string} text A rendered service preview or a service definition.
 * @returns {boolean}
 */
export function namesDaemon(text) {
  return DAEMON_ASSERTION.test(text);
}

/**
 * The real rendering of each per-user backend, recorded from the controllers.
 *
 * These are the three shapes the assertion has to accept. They are literals rather
 * than generated values on purpose: generating them would re-use the code under
 * test as its own oracle.
 *
 * @returns {Record<string, string>}
 */
export function platformSamples() {
  return {
    // systemd user unit: an `ExecStart=` directive with a bare absolute path.
    "systemd (Linux)": [
      "backend:  systemd-user",
      "service:  not_installed",
      "[Service]",
      "Type=simple",
      "ExecStart=/usr/local/install/versions/v0.1.0/bin/jarvisd",
      "Restart=on-failure",
    ].join("\n"),
    // launchd plist: the path is the text content of a `<string>` element, so
    // there is no `ExecStart=` and the path is preceded by `>` rather than `=`.
    "launchd (macOS)": [
      "backend:  launch-agent",
      "service:  not_installed",
      "    <key>ProgramArguments</key>",
      "    <array>",
      "        <string>/Users/runner/install/versions/v0.1.0/bin/jarvisd</string>",
      "    </array>",
    ].join("\n"),
    // Windows scheduled task: a quoted absolute path inside a task command line.
    "scheduled task (Windows)": [
      "backend:  windows-scheduled-task",
      "service:  not_installed",
      "effect:   Register a per-user scheduled task (no elevation).",
      'args:     /create /tn "jarvisd" /tr "C:\\install\\versions\\v0.1.0\\bin\\jarvisd.exe"',
    ].join("\n"),
  };
}

/**
 * The negative samples. Each must be **rejected**, and each is a shape that a
 * weaker assertion accepted.
 *
 * @returns {Record<string, string>}
 */
export function negativeSamples() {
  return {
    // A service pointing at the client would start and never serve.
    "a preview naming only the client": [
      "backend:  systemd-user",
      "ExecStart=/usr/local/install/versions/v0.1.0/bin/jarvis",
    ].join("\n"),
    // The trap that made the first version pass on Windows for the wrong reason:
    // the description mentions the name without being the executable.
    "a description mentioning jarvisd": [
      "backend:  systemd-user",
      'Description="JARVIS local daemon (jarvisd)"',
      "ExecStart=/usr/local/install/versions/v0.1.0/bin/jarvis",
    ].join("\n"),
  };
}

/**
 * Checks the assertion against every sample.
 *
 * @returns {string[]} One message per failure; empty when the assertion holds.
 */
export function assertionFailures() {
  const failures = [];
  for (const [platform, sample] of Object.entries(platformSamples())) {
    if (!namesDaemon(sample)) {
      failures.push(`${platform} is not recognized as naming the daemon`);
    }
  }
  for (const [name, sample] of Object.entries(negativeSamples())) {
    if (namesDaemon(sample)) {
      failures.push(`${name} was wrongly accepted`);
    }
  }
  return failures;
}
