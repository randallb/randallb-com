// Physical same-handle compatibility belongs to the cwpdb storage owner.
import assert from "node:assert/strict";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import {
  desktopStorageLibraryFilename,
  loadWlpDb,
  WlpDb,
} from "../../../apps/workloop-desktop/wlpdb.ts";

Deno.test("Desktop WlpDb and CWR facade share one native handle", () => {
  const library = loadWlpDb(
    join(
      fileURLToPath(new URL("../../../", import.meta.url)),
      "target/debug",
      desktopStorageLibraryFilename(),
    ),
  );
  const store = WlpDb.open(library, ":memory:");
  const reference = "cwp://test.local/variables/same-handle";
  try {
    const created = store.desktopOperation({
      operation: "create_variable",
      reference,
      value: { name: "SHARED", value: "one" },
    });
    assert.equal(created.outcome, "committed");
    // A separate in-memory connection cannot observe these rows. Both the
    // legacy storage transport and declared facade must use the one handle.
    const rows = store.query({
      sql: "SELECT count(*) FROM nodes WHERE kind='declared_entity'",
    }).rows;
    assert.equal(rows[0][0], 1);
    store.executeBatch([{
      sql:
        "UPDATE nodes SET revision=9007199254740993 WHERE kind='declared_entity'",
    }]);
    const read = store.desktopOperation({
      operation: "get_variable",
      reference,
    });
    assert.equal(read.outcome, "committed");
    if (read.outcome === "committed") {
      assert.equal(read.record.revision, "9007199254740993");
    }
  } finally {
    store.close();
    assert.throws(
      () => store.desktopOperation({ operation: "get_variable", reference }),
      /closed/,
    );
    library.close();
  }
});
