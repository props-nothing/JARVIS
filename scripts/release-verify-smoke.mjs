// Release verification journey (`FND-011`).
//
// This is the runnable proof that the consumer side of a release actually
// rejects modified bytes. It does NOT register anything, publish anything, or
// use a secret. It uses only the two built binaries, the committed
// **non-production test key**, and a temporary directory.
//
// The shape is deliberately "build one artifact, sign it, then try to break it",
// because a journey that only verifies a good release proves almost nothing: a
// verifier that returns success unconditionally would pass it. Every step after
// the first *requires a failure*, and the exit code is the assertion.
//
//   1. stage the built binaries as release artifacts;
//   2. build and sign a manifest with the test key;
//   3. `jarvis verify-release` must SUCCEED;
//   4. flip one artifact byte; `verify-release` must FAIL with a digest error;
//   5. rewrite the manifest body; `verify-release` must FAIL with a signature
//      error;
//   6. present a signature from an untrusted key; it must FAIL as unknown key;
//   7. restore the good artifact and require success again, so the failures are
//      attributable to the tampering and not to a broken fixture.
//
// Step 7 is what makes the earlier failures credible. Without it, a journey whose
// manifest was malformed from the start would report the same "refused" results
// and look like a pass.

import { spawnSync } from "node:child_process";
import {
  copyFileSync,
  existsSync,
  mkdtempSync,
  readFileSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import process from "node:process";

/** The committed non-production test key id, mirrored from the Rust crate. */
const TEST_KEY_ID = "jarvis-test-release-1";

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

/** Runs a command to completion and returns its result without throwing. */
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

/** Resolves the two binaries and the Cargo runner for the example signer. */
function resolveTools() {
  const directory = process.argv[2];
  if (!directory) {
    throw new Error(
      "usage: release-verify-smoke.mjs <directory containing jarvis and jarvisd>",
    );
  }
  const suffix = process.platform === "win32" ? ".exe" : "";
  const daemon = join(directory, `jarvisd${suffix}`);
  const client = join(directory, `jarvis${suffix}`);
  for (const path of [daemon, client]) {
    if (!existsSync(path)) {
      throw new Error(`missing built binary: ${path}`);
    }
  }
  return { daemon, client, suffix };
}

async function main() {
  const { daemon, client, suffix } = resolveTools();
  const directory = mkdtempSync(join(tmpdir(), "jarvis-release-"));
  console.log(`directory: ${directory}`);

  try {
    // 1. Stage the built binaries as a release directory. This mirrors what a
    //    real packaging step does — the manifest must describe the bytes that
    //    would be published, staged under their release names, not a build path.
    const stagingClient = join(directory, `jarvis${suffix}`);
    const stagingDaemon = join(directory, `jarvisd${suffix}`);
    copyFileSync(client, stagingClient);
    copyFileSync(daemon, stagingDaemon);

    const target = process.env.JARVIS_TARGET ?? "x86_64-unknown-linux-gnu";
    const version = "0.1.0";
    const artifactName = `jarvis${version}-${target}${suffix}`;

    // 2. Build and sign the manifest with the test key.
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
    if (generated.status !== 0) {
      fail("could not build and sign a test release", generated.stdout);
      return;
    }
    const manifest = join(directory, `release-${version}-${target}.json`);
    if (!existsSync(manifest) || !existsSync(`${manifest}.sig`)) {
      fail("the signer did not produce a manifest and signature", generated.stdout);
      return;
    }
    pass("the test signer produced a manifest and a detached signature");

    // The signer stages copies itself, so the manifest names what it wrote. The
    // journey reads the manifest to learn those names rather than guessing them,
    // so a naming change cannot silently make this journey vacuous.
    const described = JSON.parse(readFileSync(manifest, "utf8"));
    if (!Array.isArray(described.artifacts) || described.artifacts.length === 0) {
      fail("the signed manifest lists no artifacts", readFileSync(manifest, "utf8"));
      return;
    }
    const firstArtifact = join(directory, described.artifacts[0].file);
    if (!existsSync(firstArtifact)) {
      fail("a listed artifact is missing from the directory", described.artifacts[0].file);
      return;
    }

    // 3. A genuine signed release must verify.
    const good = run(client, ["verify-release", "--manifest", manifest]);
    if (good.status !== 0) {
      fail("a valid signed release did not verify", good.stdout);
      return;
    }
    if (!good.stdout.includes("every listed byte matches its signed digest")) {
      fail("the verifier reported success without confirming the digests", good.stdout);
      return;
    }
    if (!good.stdout.includes(TEST_KEY_ID)) {
      // A test-key verification must be visibly different from a production one.
      fail("the verifier did not disclose that it used the test key", good.stdout);
      return;
    }
    pass("a valid signed release verified, and the test key was disclosed");

    // 4. One flipped byte must be refused by the digest check.
    const original = readFileSync(firstArtifact);
    const tampered = Buffer.from(original);
    tampered[Math.floor(tampered.length / 2)] ^= 0xff;
    writeFileSync(firstArtifact, tampered);

    const byDigest = run(client, ["verify-release", "--manifest", manifest]);
    if (byDigest.status === 0) {
      fail("a modified artifact was accepted; the digest check does not falsify");
      return;
    }
    if (!byDigest.stdout.includes("jarvis.release_digest_mismatch")) {
      fail("a modified artifact failed for the wrong reason", byDigest.stdout);
      return;
    }
    pass("a single flipped artifact byte was refused (digest mismatch)");

    // 5. Rewriting the manifest body must be refused by the signature. This is
    //    the attack a digest-only scheme cannot detect: the digest matches the
    //    bytes the attacker also rewrote.
    writeFileSync(firstArtifact, original);
    const manifestText = readFileSync(manifest, "utf8");
    const rewritten = manifestText.replace(described.artifacts[0].sha256, "a".repeat(64));
    if (rewritten === manifestText) {
      fail("the manifest rewrite changed nothing, so this check is vacuous");
      return;
    }
    writeFileSync(manifest, rewritten);

    const bySignature = run(client, ["verify-release", "--manifest", manifest]);
    if (bySignature.status === 0) {
      fail("a rewritten manifest was accepted; the signature does not falsify");
      return;
    }
    if (!bySignature.stdout.includes("jarvis.release_signature_mismatch")) {
      fail("a rewritten manifest failed for the wrong reason", bySignature.stdout);
      return;
    }
    pass("a rewritten manifest body was refused (signature mismatch)");

    // 6. A signature naming an untrusted key must be refused, not merely
    //    "unverified".
    writeFileSync(manifest, manifestText);
    const envelope = readFileSync(`${manifest}.sig`, "utf8");
    const foreignKey = envelope.replace(TEST_KEY_ID, "jarvis-attacker-1");
    if (foreignKey === envelope) {
      fail("the envelope does not name the test key, so this check is vacuous");
      return;
    }
    writeFileSync(`${manifest}.sig`, foreignKey);

    const byKey = run(client, ["verify-release", "--manifest", manifest]);
    if (byKey.status === 0) {
      fail("a signature from an untrusted key was accepted");
      return;
    }
    if (!byKey.stdout.includes("jarvis.release_key_unknown")) {
      fail("an untrusted key failed for the wrong reason", byKey.stdout);
      return;
    }
    pass("a signature from an untrusted key was refused (unknown key)");

    // 7. Restore the genuine signature and artifact and require success again.
    //    This is what proves the three failures above came from the tampering
    //    rather than from a fixture that never verified in the first place.
    writeFileSync(`${manifest}.sig`, envelope);
    const restored = run(client, ["verify-release", "--manifest", manifest]);
    if (restored.status !== 0) {
      fail("the restored release did not verify, so the fixture is unreliable", restored.stdout);
      return;
    }
    pass("restoring the genuine bytes restored verification");

    // `daemon` is resolved so a broken build is caught here rather than silently
    // skipped; the verification itself needs only the client.
    if (!existsSync(daemon)) {
      fail("the daemon binary is missing, so the release would be incomplete");
    } else {
      pass("the release includes a daemon binary");
    }

    console.log(`artifact: ${artifactName}`);
  } catch (error) {
    fail(error.message);
  } finally {
    rmSync(directory, { recursive: true, force: true });
  }

  if (process.exitCode === 1) {
    console.error("\nrelease verification journey FAILED");
  } else {
    console.log("\nrelease verification journey passed");
  }
}

await main();
