// Dev/test Postgres without Docker: PGlite (Postgres 18 compiled to WASM) behind
// the Postgres wire protocol. In-memory; every start is a fresh database.
//   node server.mjs [port]   →   BF_DATABASE_URL=postgres://postgres@127.0.0.1:<port>/postgres?sslmode=disable
// Not for production: one backend, so clients are served one connection at a time
// (others wait), and LISTEN/NOTIFY across connections does not work.
import net from "node:net";
import { PGlite } from "@electric-sql/pglite";
import { pg_trgm } from "@electric-sql/pglite/contrib/pg_trgm";
import { PGLiteSocketHandler } from "@electric-sql/pglite-socket";

const port = Number(process.argv[2] ?? 54329);
const db = await PGlite.create({ extensions: { pg_trgm } });

// All connections share PGlite's single backend, but each client numbers its prepared
// statements from 1 (sqlx_s_1, ...). Serving connections one at a time and dropping
// statements in between keeps their names from colliding.
let queue = Promise.resolve();

// The handler expects pglite-socket's (unexported) query queue. With one client at a
// time there is nothing to interleave, so each message goes straight to PGlite.
const queryQueue = {
  enqueue: (_id, message, onData) =>
    db.runExclusive(() => db.execProtocolRawStream(message, { onRawData: onData })),
  clearQueueForHandler() {},
  async clearTransactionIfNeeded() {
    if (db.isInTransaction()) await db.exec("ROLLBACK");
  },
};

const server = net.createServer((socket) => {
  socket.pause();
  queue = queue.then(
    () =>
      new Promise(async (done) => {
        if (socket.destroyed) return done();
        await db.exec("DEALLOCATE ALL");
        const handler = new PGLiteSocketHandler({ queryQueue });
        socket.once("close", () => done());
        await handler.attach(socket);
        socket.resume();
      }),
  );
});

server.listen(port, "127.0.0.1", () => console.log(`devdb listening on 127.0.0.1:${port}`));

const stop = async () => {
  server.close();
  await db.close();
  process.exit(0);
};
process.on("SIGINT", stop);
process.on("SIGTERM", stop);
