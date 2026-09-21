// Checks the clean-machine service assertion against the three real platform
// renderings, so a fix for one platform cannot break another. This runs on the
// authoring host without needing the platform: it asserts on the string shapes
// the controllers actually produce.
//
// It exists because the assertion has now been wrong twice in the same way — it
// keyed on one platform's formatting. Windows passed, Linux failed, and macOS
// failed once the native lane actually started running.

const assertion = /[\\/]jarvisd(\.exe)?\b/;

const samples = {
  // systemd user unit: an `ExecStart=` directive with a bare absolute path.
  "systemd (Linux)": [
    "backend:  systemd-user",
    "service:  not_installed",
    "[Service]",
    "Type=simple",
    "ExecStart=/usr/local/install/versions/v0.1.0/bin/jarvisd",
    "Restart=on-failure",
  ].join("\n"),
  // launchd plist: the path is the text content of a `<string>` element, so there
  // is no `ExecStart=` and the path is preceded by `>` rather than `=`.
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

let failures = 0;
for (const [platform, sample] of Object.entries(samples)) {
  const matched = assertion.test(sample);
  console.log(`${matched ? "ok  " : "FAIL"} ${platform} is recognized as naming the daemon`);
  if (!matched) {
    failures += 1;
  }
}

// The negative case matters as much as the positive ones: a preview that named
// only the client would start and never serve, so it must not be accepted.
const clientOnly = [
  "backend:  systemd-user",
  "ExecStart=/usr/local/install/versions/v0.1.0/bin/jarvis",
].join("\n");
if (assertion.test(clientOnly)) {
  console.log("FAIL a preview naming only the client was accepted");
  failures += 1;
} else {
  console.log("ok   a preview naming only the client is rejected");
}

// The description mentions the name without being the executable. A bare-name
// assertion would pass on this text, which is why the check requires a path.
const descriptionOnly = [
  "backend:  systemd-user",
  'Description="JARVIS local daemon (jarvisd)"',
  "ExecStart=/usr/local/install/versions/v0.1.0/bin/jarvis",
].join("\n");
if (assertion.test(descriptionOnly)) {
  console.log("FAIL a description mentioning jarvisd was accepted as the executable");
  failures += 1;
} else {
  console.log("ok   a description mention is not mistaken for the executable");
}

if (failures > 0) {
  console.error(`\n${failures} assertion case(s) failed`);
  process.exitCode = 1;
} else {
  console.log("\nservice assertion holds on all three platforms");
}
