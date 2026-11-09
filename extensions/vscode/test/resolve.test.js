const { test } = require("node:test");
const assert = require("node:assert/strict");
const { resolveBinary } = require("../src/resolve.js");

/** An environment where nothing is executable, for tests to override piecemeal. */
function env(overrides = {}) {
  return {
    path: "/usr/bin:/usr/local/bin",
    home: "/home/tester",
    windows: false,
    isExecutable: () => false,
    ...overrides,
  };
}

test("a configured path wins", () => {
  const found = resolveBinary("/opt/cmplx", env({ isExecutable: (p) => p === "/opt/cmplx" }));
  assert.equal(found, "/opt/cmplx");
});

test("a configured path that is not executable resolves to undefined", () => {
  // Not a fallthrough: a typo must surface, not silently use PATH's copy.
  const found = resolveBinary(
    "/opt/typo",
    env({ isExecutable: (p) => p === "/usr/local/bin/cmplx" }),
  );
  assert.equal(found, undefined);
});

test("PATH is searched when nothing is configured", () => {
  const found = resolveBinary("", env({ isExecutable: (p) => p === "/usr/local/bin/cmplx" }));
  assert.equal(found, "/usr/local/bin/cmplx");
});

test("a blank configured path is treated as unset", () => {
  const found = resolveBinary("   ", env({ isExecutable: (p) => p === "/usr/bin/cmplx" }));
  assert.equal(found, "/usr/bin/cmplx");
});

test("the cargo bin is the fallback when PATH has no hit", () => {
  const found = resolveBinary(
    "",
    env({ isExecutable: (p) => p === "/home/tester/.cargo/bin/cmplx" }),
  );
  assert.equal(found, "/home/tester/.cargo/bin/cmplx");
});

test("nothing anywhere resolves to undefined", () => {
  assert.equal(resolveBinary("", env()), undefined);
});

test("windows looks for cmplx.exe, on PATH and in the cargo bin", () => {
  const onPath = resolveBinary(
    "",
    env({
      windows: true,
      path: "C:\\bin;C:\\tools",
      isExecutable: (p) => p === "C:\\tools\\cmplx.exe",
    }),
  );
  assert.equal(onPath, "C:\\tools\\cmplx.exe");

  const inCargo = resolveBinary(
    "",
    env({
      windows: true,
      path: "C:\\bin",
      home: "C:\\Users\\tester",
      isExecutable: (p) => p === "C:\\Users\\tester\\.cargo\\bin\\cmplx.exe",
    }),
  );
  assert.equal(inCargo, "C:\\Users\\tester\\.cargo\\bin\\cmplx.exe");
});

test("an empty PATH is not a crash", () => {
  assert.equal(resolveBinary("", env({ path: "" })), undefined);
});
