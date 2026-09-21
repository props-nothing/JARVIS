// Checks the clean-machine service assertion against the three real platform
// renderings, so a fix for one platform cannot break another. This runs on the
// authoring host without needing the platform: it asserts on the string shapes
// the controllers actually produce.
//
// It exists because the assertion has now been wrong twice in the same way — it
// keyed on one platform's formatting. Windows passed, Linux failed, and macOS
// failed once the native lane actually started running.

// The matcher itself lives in `daemon-assertion.mjs` and is imported here, so this
// check and the journey that uses it cannot test two different regexes. It was a
// duplicated literal before, which is a guard that can silently stop guarding.

import { assertionFailures, negativeSamples, platformSamples } from "./daemon-assertion.mjs";

const failures = assertionFailures();

for (const platform of Object.keys(platformSamples())) {
  const failed = failures.some((failure) => failure.startsWith(platform));
  console.log(`${failed ? "FAIL" : "ok  "} ${platform} is recognized as naming the daemon`);
}

// The negative cases matter as much as the positive ones, and each one is a shape
// a weaker assertion accepted: a client-only preview, and a *description* that
// mentions the name without being the executable.
for (const name of Object.keys(negativeSamples())) {
  const failed = failures.some((failure) => failure.startsWith(name));
  console.log(`${failed ? "FAIL" : "ok  "} ${name} is rejected`);
}

if (failures.length > 0) {
  console.error(`\n${failures.length} assertion case(s) failed`);
  for (const failure of failures) {
    console.error(`  ${failure}`);
  }
  process.exitCode = 1;
} else {
  console.log("\nservice assertion holds on all three platforms");
}
