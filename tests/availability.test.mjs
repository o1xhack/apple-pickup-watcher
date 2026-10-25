import assert from "node:assert/strict";
import test from "node:test";
import {loadSource} from "./load-source.mjs";
const {describeAvailability, isUntrusted} = loadSource("src/lib/types.ts");
test("Apple pickup not open is a pending state with the original quote, never out of stock or a failure", () => {
  const status = {kind:"unknown", reason:"pickup_not_open", message:"Check back for availability on 10.24"};
  assert.equal(isUntrusted(status), false);
  const display = describeAvailability(status);
  assert.equal(display.label, "未开放");
  assert.equal(display.tone, "pending");
  assert.ok(display.detail.includes("10.24"));
});
