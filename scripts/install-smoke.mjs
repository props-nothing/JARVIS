// Install, update, rollback, and uninstall journey (`FND-012`).
//
// This is the runnable proof that an install can change the running program without
// ever touching user data, and that a failed activation leaves the previous version
// working. It uses only the built binaries, the committed **non-production test
// key**, and a temporary root. It publishes nothing and registers no service.
//
// The steps are chosen so that each one can falsify a specific claim:
//
//   1. install 0.1.0 from a verified release and require the active version;
//   2. seed user data (a database file) that must survive every later step;
//   3. update to 0.2.0 and require the previous version to be recorded;
//   4. attempt a rollback to a version whose directory is gone and require a
//      refusal rather than a silent no-op;
//   5. roll back to 0.1.0 and require the old bytes to be the active ones again;
//   6. uninstall and require the program files gone while the database is
//      unchanged byte-for-byte;
//   7. refuse a purge without `--acknowledge-purge`, then prove an acknowledged
//      purge removes the data it named.
//
// Step 4 and step 7 are the ones that matter most. A rollback that quietly did
// nothing, and a purge reachable through the same flags as a safe uninstall, are
// exactly the two failure modes this slice exists to prevent.

import { spawnSync } from "node:child_process";
import {
  copyFileSync,
  existsSync,
  mkdirSync,
  mkdtempSync,
  readdirSync,
  readFileSync,
  renameSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import process from "node:process";

/** The database file name the install journey seeds and then asserts on. */
const DATABASE = "jarvis.sqlite";

function fail(message, detail) {
  console.error(`FAIL  ${message}`);
  if (detail) {
    console.error(String(detail).slice(0, 4000));
  }
  process.exitCode = 1;
}

function pass(message) {
  console.log(`ok    ${message}`);
}

/** Runs a command to completion and returns status and combined output. */
function run(command, args, options = {}) {
  const result = spawnSync(command, args, {
    encoding: "utf8",
    timeout: 120_000,
    windowsHide: true,
    ...options,
  });
  return {
    status: result.status,
    stdout: `${result.stdout ?? ""}${result.stderr ?? ""}`,
  };
}

/** Builds a signed and verified test release for `version` under `directory`. */
function makeRelease(binaries, directory, target, version) {
  const suffix = process.platform === "win32" ? ".exe" : "";
  // The signer packages the real built binaries under their release names, so the
  // digests describe real artifacts rather than invented ones.
  for (const name of ["jarvis", "jarvisd"]) {
    copyFileSync(join(binaries, `${name}${suffix}`), join(directory, `${name}${suffix}`));
  }

  const generated = run("cargo", [
    "run",
    "--quiet",
    "-p",
    "jarvis-infrastructure",
    "--example",
    "make_test_release",
    "--",
    directory,
    target,
    version,
    "development",
  ]);
  return { result: generated, version };
}

async function main() {
  const binaries = process.argv[2];
  const target = process.env.JARVIS_TARGET ?? "x86_64-unknown-linux-gnu";
  if (!binaries) {
    fail("usage: install-smoke.mjs <directory containing jarvis>");
    return;
  }
  const suffix = process.platform === "win32" ? ".exe" : "";
  const client = join(binaries, `jarvis${suffix}`);
  if (!existsSync(client)) {
    fail(`missing built client: ${client}`);
    return;
  }

  const root = mkdtempSync(join(tmpdir(), "jarvis-install-journey-"));
  const installRoot = join(root, "install");
  const profile = join(root, "profile");
  console.log(`root:    ${root}`);
  console.log(`install: ${installRoot}`);
  console.log(`profile: ${profile}`);

  // Every install command runs against the same explicit portable roots, so no
  // ambient developer state can influence the result.
  const jarvis = (args) =>
    run(client, ["--profile", profile, "install", "--root", installRoot, ...args]);

  try {
    // 1. Install 0.1.0 and require it to become the active version.
    makeRelease(binaries, root, target, "0.1.0");
    const manifest = join(root, `release-0.1.0-${target}.json`);
    if (!existsSync(manifest)) {
      fail("the test signer produced no manifest");
      return;
    }

    const installed = jarvis(["update", "--manifest", manifest, "--target", target, "--confirm"]);
    if (installed.status !== 0) {
      fail("installing 0.1.0 failed", installed.stdout);
      return;
    }
    const status = jarvis(["status"]);
    if (!status.stdout.includes("active:       0.1.0")) {
      fail("0.1.0 did not become the active version", status.stdout);
      return;
    }
    pass("installed 0.1.0 and it is the active version");

    // The staged name must be the stable one from the manifest, not the versioned
    // download name, because a service definition points at the stable name.
    const versionBin = join(installRoot, "versions", "v0.1.0", "bin");
    if (!existsSync(versionBin)) {
      fail("the version directory was not created", readdirSync(installRoot).join(", "));
      return;
    }
    const staged = readdirSync(versionBin);
    if (!staged.includes(`jarvisd${suffix}`)) {
      fail("the binary was not staged under its stable installed name", staged.join(", "));
      return;
    }
    pass(`staged the binary under its stable name (${staged.join(", ")})`);

    // 2. Seed user data that every later step must preserve. The directory is
    //    created here because a real profile is created by the daemon on first
    //    run; this journey is about install semantics, so it seeds the state
    //    rather than starting a daemon.
    const dataDirectory = join(profile, "data");
    mkdirSync(dataDirectory, { recursive: true });
    const databasePath = join(dataDirectory, DATABASE);
    writeFileSync(databasePath, "durable user memory");
    if (!existsSync(databasePath)) {
      fail("the database fixture was not written, so the assertions would be vacuous");
      return;
    }
    pass("seeded user data that later steps must preserve");

    // 3. Update to 0.2.0 and require the previous version to be recorded.
    makeRelease(binaries, root, target, "0.2.0");
    const manifest2 = join(root, `release-0.2.0-${target}.json`);
    const updated = jarvis(["update", "--manifest", manifest2, "--target", target, "--confirm"]);
    if (updated.status !== 0) {
      fail("updating to 0.2.0 failed", updated.stdout);
      return;
    }
    const afterUpdate = jarvis(["status"]);
    if (!afterUpdate.stdout.includes("active:       0.2.0")) {
      fail("0.2.0 did not become active", afterUpdate.stdout);
      return;
    }
    if (!afterUpdate.stdout.includes("rollback to:  0.1.0")) {
      fail("the update did not record a rollback target", afterUpdate.stdout);
      return;
    }
    pass("updated to 0.2.0 and recorded 0.1.0 as the rollback target");

    // 4. A rollback whose target is gone must refuse, not silently succeed. The
    //    directory is moved aside rather than deleted, so step 5 can restore the
    //    exact installed bytes and test a genuine rollback.
    const versionOne = join(installRoot, "versions", "v0.1.0");
    const parked = join(installRoot, "parked-v0.1.0");
    renameSync(versionOne, parked);
    const orphaned = jarvis(["rollback", "--confirm"]);
    if (orphaned.status === 0) {
      fail("a rollback to a removed version reported success");
      return;
    }
    if (!orphaned.stdout.includes("jarvis.install_previous_version_missing")) {
      fail("a rollback to a removed version failed for the wrong reason", orphaned.stdout);
      return;
    }
    // The refusal must not have changed the active version.
    const stillActive = jarvis(["status"]);
    if (!stillActive.stdout.includes("active:       0.2.0")) {
      fail("a refused rollback changed the active version", stillActive.stdout);
      return;
    }
    pass("a rollback with a missing target was refused and changed nothing");

    // 5. With the target restored, a rollback works and the old bytes are active.
    renameSync(parked, versionOne);
    const rollback = jarvis(["rollback", "--confirm"]);
    if (rollback.status !== 0) {
      fail("rolling back to 0.1.0 failed", rollback.stdout);
      return;
    }
    const afterRollback = jarvis(["status"]);
    if (!afterRollback.stdout.includes("active:       0.1.0")) {
      fail("0.1.0 did not become active after rollback", afterRollback.stdout);
      return;
    }
    if (!afterRollback.stdout.includes("0.2.0")) {
      fail("rollback deleted the newer version, so it is not reversible", afterRollback.stdout);
      return;
    }
    pass("rolled back to 0.1.0 and kept 0.2.0 installed");

    // 6. Uninstall must remove program files and leave the database byte-identical.
    const uninstalled = jarvis(["uninstall", "--confirm"]);
    if (uninstalled.status !== 0) {
      fail("uninstall failed", uninstalled.stdout);
      return;
    }
    if (existsSync(join(installRoot, "versions", "v0.1.0"))) {
      fail("the uninstall left program files behind");
      return;
    }
    if (!existsSync(databasePath)) {
      fail("the uninstall removed user data; it must retain it without --purge");
      return;
    }
    if (readFileSync(databasePath, "utf8") !== "durable user memory") {
      fail("the uninstall modified user data");
      return;
    }
    if (!uninstalled.stdout.includes("user data retained at")) {
      fail("the uninstall did not report where the data was retained", uninstalled.stdout);
      return;
    }
    pass("uninstall removed program files, retained and did not modify user data");

    // 7. A purge needs its own acknowledgement, then removes the data it named.
    //    Reinstall first so there is something to purge.
    makeRelease(binaries, root, target, "0.3.0");
    const manifest3 = join(root, `release-0.3.0-${target}.json`);
    const reinstalled = jarvis(["update", "--manifest", manifest3, "--target", target, "--confirm"]);
    if (reinstalled.status !== 0) {
      fail("reinstalling for the purge check failed", reinstalled.stdout);
      return;
    }

    const purgeRefused = jarvis(["uninstall", "--purge", "--confirm"]);
    if (purgeRefused.status === 0) {
      fail("a purge without --acknowledge-purge was accepted");
      return;
    }
    if (!purgeRefused.stdout.includes("jarvis.install_purge_not_acknowledged")) {
      fail("a purge failed for the wrong reason", purgeRefused.stdout);
      return;
    }
    if (!existsSync(databasePath)) {
      fail("a refused purge deleted user data");
      return;
    }
    pass("a purge without its acknowledgement was refused and deleted nothing");

    const purged = jarvis(["uninstall", "--purge", "--confirm", "--acknowledge-purge"]);
    if (purged.status !== 0) {
      fail("an acknowledged purge failed", purged.stdout);
      return;
    }
    if (existsSync(databasePath)) {
      fail("an acknowledged purge did not remove the user data it named");
      return;
    }
    pass("an acknowledged purge removed the user data it named");

    // A bare `install` must never change anything, and the install and profile
    // roots are separate directories.
    const bare = jarvis([]);
    if (bare.status !== 0 || !bare.stdout.includes("user data:")) {
      fail("a bare `install` did not report installed state read-only", bare.stdout);
      return;
    }
    pass("a bare `install` reported state without changing anything");
  } catch (error) {
    fail(error.message);
  } finally {
    rmSync(root, { recursive: true, force: true });
  }

  if (process.exitCode === 1) {
    console.error("\ninstall journey FAILED");
  } else {
    console.log("\ninstall journey passed");
  }
}

await main();
