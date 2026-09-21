// Clean-machine smoke journey.
//
// This does what `ACC-001` describes, using only the two built binaries and an
// empty directory: start the per-user daemon, run `jarvis status` and
// `jarvis doctor`, stop it, then start it again against the same profile to prove
// state survives a restart. It deliberately does NOT register a real service:
// native service registration mutates the runner's logon state and is asserted
// separately.
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
import { existsSync, mkdtempSync, readdirSync, rmSync, statSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
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
    const service = run(client, ["--profile", profile, "service", "show"]);
    if (service.status !== 0) {
      fail("the service preview failed", `${service.stdout}${service.stderr}`);
    } else if (!/jarvisd(\.exe)?"/.test(service.stdout)) {
      fail("the service preview does not name the jarvisd daemon", service.stdout);
    } else if (!/no elevation/.test(service.stdout)) {
      fail("the service preview does not state the no-elevation mode", service.stdout);
    } else {
      pass("service: the preview names jarvisd and states the per-user mode");
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
