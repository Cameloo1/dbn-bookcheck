# dbn-es-bench Node binding

This package exposes the Rust core's file-backed MBO decoder, decode statistics, and stateful liquidity-sweep scanner through Node-API.

```ts
import { MboDecoder, decodeStats } from "dbn-es-bench";

const path = "session-mbo.dbn.zst";
console.log(decodeStats(path));

const decoder = new MboDecoder(path);
for (;;) {
  const record = decoder.nextRecord();
  if (record === null) break;
  // Consume one owned record; the remainder stays in the native stream.
}
```

All DBN identifiers, prices, timestamps, counts, and durations represented as Rust `u64` or `i64` cross the boundary as JavaScript `bigint`. The package requires Node.js 18 or newer and is configured for Windows x64, Linux x64 glibc, and Intel/Apple Silicon macOS artifacts. Only artifacts tested on their claimed target should be published.
