import test = require("node:test");

import { assertLiveProjectionMatchesManifest } from "./semantic-corpus.js";

test("public artifact projection is derived from the canonical identity manifest", () => {
  assertLiveProjectionMatchesManifest();
});
