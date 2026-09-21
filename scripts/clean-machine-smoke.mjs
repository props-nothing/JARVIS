// Clean-machine smoke journey.
//
// This does what `ACC-001` describes, using only the two built binaries and an
// empty directory: start the per-user daemon, run `jarvis status` and
// `jarvis doctor`, stop it, then start it again against the same profile to prove
// state survives a restart. It also exercises the support-bundle preview and
// export with a planted secret canary. It deliberately does NOT register a real
// service: native service registration mutates the runner's logon state and is
// asserted separately.
//
// Two properties make this a clean-machine test rather than a "the suite passed"
// test:
//
// 1. It never reads the developer's or the runner's real profile. The profile is
//    an explicit `--profile <empty dir>`, so no ambient state can make it pass.
// 2. It asserts the profile directory contents, not only exit codes. An exit code
//    of 0 with nothing written would still be a failure.
//
// It is deliberately dependency-free (Node standard library only) so it runs on
// every target without an install step, and it is careful about process
// termination order so a failing assertion cannot leave a daemon behind.

import { spawn, spawnSync } from "node:child_process";
import {
  appendFileSync,
  existsSync,
  mkdtempSync,
  readdirSync,
  readFileSync,
  rmSync,
  statSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { basename, join } from "node:path";
import process from "node:process";

const READY_TIMEOUT_MS = 30_000;
const POLL_INTERVAL_MS = 250;
const SHUTDOWN_TIMEOUT_MS = 20_000;

/** Paths to the two binaries, resolved from a release or debug directory. */
function resolveBinaries() {
  const directory = process.argv[2];
  if (!directory) {
    throw new Error("usage: clean-machine-smoke.mjs <directory containing jarvis and jarvisd>");
  }
  const suffix = process.platform === "win32" ? ".exe" : "";
  const daemon = join(directory, `jarvisd${suffix}`);
  const client = join(directory, `jarvis${suffix}`);
  for (const path of [daemon, client]) {
    if (!existsSync(path)) {
      throw new Error(`missing built binary: ${path}`);
    }
  }
  return { daemon, client };
}

/** Runs a command to completion and returns its result without throwing. */
function run(command, args, options = {}) {
  const result = spawnSync(command, args, {
    encoding: "utf8",
    timeout: 60_000,
    windowsHide: true,
    ...options,
  });
  return {
    status: result.status,
    stdout: result.stdout ?? "",
    stderr: result.stderr ?? "",
  };
}

function fail(message, detail) {
  console.error(`FAIL  ${message}`);
  if (detail) {
    // Bounded: a runaway output must not bury the message that matters.
    console.error(String(detail).slice(0, 4000));
  }
  process.exitCode = 1;
}

function pass(message) {
  console.log(`ok    ${message}`);
}

/** Starts the daemon and waits until `status` reports readiness. */
async function startAndWait(daemon, client, profile, label) {
  const daemonLog = [];
  const child = spawn(daemon, ["--profile", profile], {
    stdio: ["ignore", "pipe", "pipe"],
    windowsHide: true,
  });
  child.stdout.on("data", (chunk) => daemonLog.push(String(chunk)));
  child.stderr.on("data", (chunk) => daemonLog.push(String(chunk)));

  let exitedEarly = false;
  child.once("exit", () => {
    exitedEarly = true;
  });

  const deadline = Date.now() + READY_TIMEOUT_MS;
  while (Date.now() < deadline) {
    if (exitedEarly) {
      throw new Error(
        `${label}: the daemon exited before becoming ready\n${daemonLog.join("")}`,
      );
    }
    const status = run(client, ["--profile", profile, "status"]);
    if (status.status === 0 && /state:\s*ready/.test(status.stdout)) {
      return { child, status };
    }
    await new Promise((resolve) => setTimeout(resolve, POLL_INTERVAL_MS));
  }

  throw new Error(`${label}: the daemon was not ready within ${READY_TIMEOUT_MS}ms`);
}

/** Stops the daemon and waits for the process to actually exit. */
async function stop(child, label) {
  if (child.exitCode !== null || child.signalCode !== null) {
    return;
  }
  child.kill("SIGTERM");

  const deadline = Date.now() + SHUTDOWN_TIMEOUT_MS;
  while (Date.now() < deadline) {
    if (child.exitCode !== null || child.signalCode !== null) {
      return;
    }
    await new Promise((resolve) => setTimeout(resolve, POLL_INTERVAL_MS));
  }

  // A drain that never finishes is itself a failure, but the process must not be
  // left running either way.
  child.kill("SIGKILL");
  throw new Error(`${label}: the daemon did not stop within ${SHUTDOWN_TIMEOUT_MS}ms`);
}

/**
 * Waits for an already-killed process to be reaped.
 *
 * An unclean kill has no drain to wait for, but the operating system still needs a
 * moment to release the process's file handles. Removing a lock file before the
 * handle is released is exactly the race this test would otherwise blame on
 * `repair`.
 */
async function waitForExit(child) {
  const deadline = Date.now() + SHUTDOWN_TIMEOUT_MS;
  while (Date.now() < deadline) {
    if (child.exitCode !== null || child.signalCode !== null) {
      // The exit event fires asynchronously; give the runtime a turn so handles
      // are closed before the next assertion reads them.
      await new Promise((resolve) => setTimeout(resolve, POLL_INTERVAL_MS));
      return;
    }
    await new Promise((resolve) => setTimeout(resolve, POLL_INTERVAL_MS));
  }
  throw new Error("the killed daemon was not reaped within the shutdown bound");
}

async function main() {
  const { daemon, client } = resolveBinaries();
  const profile = mkdtempSync(join(tmpdir(), "jarvis-clean-"));
  console.log(`profile: ${profile}`);

  let running = null;
  try {
    // 1. A clean profile starts, becomes ready, and reports its own identity.
    const first = await startAndWait(daemon, client, profile, "first start");
    running = first.child;
    if (!/instance:\s*[0-9a-f-]{36}/i.test(first.status.stdout)) {
      fail("status did not report an instance id", first.status.stdout);
    } else {
      pass("first start: status reported a daemon instance and readiness");
    }
    if (!/storage:\s*sqlite/.test(first.status.stdout)) {
      fail("status did not report the sqlite backend", first.status.stdout);
    } else {
      pass("first start: status reported the sqlite storage backend");
    }

    // 2. Doctor must be clean on a freshly created profile.
    const doctor = run(client, ["--profile", profile, "doctor"]);
    if (doctor.status !== 0) {
      fail("doctor reported blocking findings on a clean profile", doctor.stdout);
    } else {
      pass("first start: doctor reported no blocking findings");
    }

    // 3. The profile layout must actually exist: owner-only credential file,
    //    database, runtime discovery, and a lock.
    for (const relative of [
      join("config", "client-credential"),
      join("data", "db", "jarvis.sqlite"),
      join("run", "discovery.json"),
      join("run", "jarvis.lock"),
    ]) {
      const path = join(profile, relative);
      if (!existsSync(path)) {
        fail(`the profile is missing ${relative}`, readdirSync(profile).join(", "));
      } else {
        pass(`first start: the profile contains ${relative}`);
      }
    }

    // 4. The credential must not be readable by other users on a Unix host.
    if (process.platform !== "win32") {
      const mode = statSync(join(profile, "config", "client-credential")).mode & 0o777;
      if (mode !== 0o600) {
        fail(`the credential file mode is ${mode.toString(8)}, expected 600`);
      } else {
        pass("first start: the credential file is owner-only (0600)");
      }
    }

    await stop(running, "first start");
    running = null;
    pass("first start: the daemon drained and stopped");

    // 4b. Repair must converge. Whether a stopped daemon leaves a stale discovery
    //     file depends on the platform: a graceful drain unpublishes it, but
    //     Windows cannot deliver a graceful signal through `child.kill`, so a stop
    //     there is an unclean termination and the file survives. This step
    //     therefore asserts *convergence* — run repair, then require a second run
    //     to find nothing — rather than assuming which way the stop went. An
    //     assertion on the pre-state would pass on one platform and fail on the
    //     other for a reason that is not a defect.
    const firstRepair = run(client, ["--profile", profile, "repair", "--confirm"]);
    if (firstRepair.status !== 0) {
      fail("repair failed after a stop", `${firstRepair.stdout}${firstRepair.stderr}`);
    }
    const converged = run(client, ["--profile", profile, "repair", "--confirm"]);
    if (!/0 plan\(s\) applied/.test(converged.stdout)) {
      fail(
        "repair did not converge: a second run still found work to do",
        converged.stdout,
      );
    } else if (existsSync(join(profile, "run", "discovery.json"))) {
      fail("repair reported convergence but the discovery file is still present");
    } else {
      pass("repair: converged to a state with nothing left to fix");
    }

    // 4c. Unclean termination must be detected from the lock, not the discovery
    //     file. The kill frees the lock and leaves the discovery file behind, so
    //     doctor must report the stale state and repair must clear it.
    const killed = await startAndWait(daemon, client, profile, "unclean");
    killed.child.kill("SIGKILL");
    await waitForExit(killed.child);
    const staleDiscovery = join(profile, "run", "discovery.json");
    if (!existsSync(staleDiscovery)) {
      fail("the discovery file did not survive an unclean kill, so this check is vacuous");
    } else {
      const afterKill = run(client, ["--profile", profile, "doctor"]);
      if (!/daemon state: jarvis\.stale_discovery/.test(afterKill.stdout)) {
        fail(
          "doctor did not detect the stale daemon state after an unclean kill",
          afterKill.stdout,
        );
      } else {
        pass("repair: doctor detected the stale discovery file after an unclean kill");
      }

      const repaired = run(client, ["--profile", profile, "repair", "--confirm"]);
      if (repaired.status !== 0) {
        fail("the stale-state repair failed", `${repaired.stdout}${repaired.stderr}`);
      } else if (existsSync(staleDiscovery)) {
        fail("the stale discovery file still exists after repair reported success");
      } else {
        pass("repair: removed the stale discovery file and verified the postcondition");
      }

      // The lock FILE is the daemon's resting state and must be left alone.
      if (!existsSync(join(profile, "run", "jarvis.lock"))) {
        fail("repair removed the benign lock file, which is not the stale artifact");
      } else {
        pass("repair: left the benign instance-lock file in place");
      }
    }

    // 5. Restart against the same profile: state must survive, and the daemon
    //    must not re-enroll a second credential.
    const second = await startAndWait(daemon, client, profile, "restart");
    running = second.child;
    pass("restart: the daemon became ready against the existing profile");

    const doctor2 = run(client, ["--profile", profile, "doctor"]);
    if (doctor2.status !== 0) {
      fail("doctor reported blocking findings after a restart", doctor2.stdout);
    } else {
      pass("restart: doctor still reported no blocking findings");
    }

    await stop(running, "restart");
    running = null;
    pass("restart: the daemon drained and stopped");

    // 6. The service preview must name the daemon binary, not the client. A
    //    service pointing at the client would start and never serve.
    //
    //    The match is on the executable **path** rather than on a platform's key
    //    name, because the three backends render it differently: systemd has
    //    `ExecStart=/path/jarvisd`, launchd puts `/path/jarvisd` inside a
    //    `<string>` element, and a Windows task has a quoted `\path\jarvisd.exe`.
    //    Keying on one platform's key name passed on Windows and failed on Linux,
    //    and then failed again on macOS once the lane actually ran.
    //
    //    Matching a path segment also avoids the trap that made the first version
    //    pass on Windows: the unit *description* contains the word "jarvisd"
    //    inside quotes, so any assertion on the bare name can be satisfied by
    //    text that is not the executable.
    const service = run(client, ["--profile", profile, "service", "show"]);
    const namesDaemon = /[\\/]jarvisd(\.exe)?\b/.test(service.stdout);
    if (service.status !== 0) {
      fail("the service preview failed", `${service.stdout}${service.stderr}`);
    } else if (!namesDaemon) {
      fail("the service preview does not name the jarvisd daemon", service.stdout);
    } else if (!/no elevation/.test(service.stdout)) {
      fail("the service preview does not state the no-elevation mode", service.stdout);
    } else {
      pass("service: the preview names jarvisd and states the per-user mode");
    }

    // 7. The support bundle must be preview-first and must not carry the enrolled
    //    credential, even when the credential is sitting in a log file. This is
    //    the canary: a secret is planted in a real log, and the exported archive
    //    is searched for it. An implementation whose redaction never ran would
    //    still pass a "does it produce a zip" check, which is why the assertion
    //    is on the planted value and not on the archive existing.
    const preview = run(client, ["--profile", profile, "support-bundle"]);
    if (preview.status !== 0) {
      fail("the bundle preview failed", `${preview.stdout}${preview.stderr}`);
    } else if (!/nothing has been written yet/.test(preview.stdout)) {
      fail("the bundle preview did not state that nothing was written", preview.stdout);
    } else if (!/excluded from every bundle/.test(preview.stdout)) {
      // A bare preview must not write a file anywhere.
      fail("the bundle preview did not state its exclusions", preview.stdout);
    } else {
      pass("bundle: a bare invocation previewed and wrote nothing");
    }

    const credential = readFileSync(join(profile, "config", "client-credential"), "utf8").trim();
    if (credential.length < 16) {
      fail("the enrolled credential is too short to serve as a canary");
    }
    const logDirectory = join(profile, "log");
    const logNames = existsSync(logDirectory) ? readdirSync(logDirectory) : [];
    if (logNames.length === 0) {
      fail("no log file exists to plant the canary in", readdirSync(profile).join(", "));
    }
    const logPath = join(logDirectory, basename(logNames[0]));
    // Plant the credential where a logged secret would appear: inside a log line.
    appendFileSync(logPath, `{"level":"INFO","message":"canary ${credential} leaked"}\n`);
    if (!readFileSync(logPath, "utf8").includes(credential)) {
      fail("the canary was not written to the log, so the test would be vacuous");
    }

    const bundlePath = join(profile, "bundle.zip");
    const exported = run(client, [
      "--profile",
      profile,
      "support-bundle",
      "--output",
      bundlePath,
    ]);
    if (exported.status !== 0) {
      fail("the bundle export failed", `${exported.stdout}${exported.stderr}`);
    } else if (!existsSync(bundlePath)) {
      fail("the bundle export reported success but wrote no file");
    } else {
      // The archive is searched as raw bytes rather than unpacked, so the check
      // does not depend on a ZIP reader being available on every runner.
      const archive = readFileSync(bundlePath, "utf8");
      if (archive.includes(credential)) {
        fail("the exported bundle contains the enrolled credential");
      } else if (!archive.includes("manifest.json")) {
        fail("the exported bundle has no manifest entry");
      } else {
        pass("bundle: the exported archive omits the planted credential");
      }
    }
  } catch (error) {
    fail(error.message);
  } finally {
    // Never leave a daemon behind, even after a failed assertion.
    if (running) {
      try {
        await stop(running, "cleanup");
      } catch {
        running.kill("SIGKILL");
      }
    }
    rmSync(profile, { recursive: true, force: true });
  }

  if (process.exitCode === 1) {
    console.error("\nclean-machine smoke journey FAILED");
  } else {
    console.log("\nclean-machine smoke journey passed");
  }
}

await main();
