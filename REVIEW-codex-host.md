# Deep code review: `host/`

Reviewed 2026-09-05 against the working tree at HEAD `5a29aef86d1dcf005d0c18342ce1b1a844455481`.

## Summary and scope

`host/` provides the JavaScript port's shared rules, origin checks, byte utilities, storage keys, and four storage adapters. I reviewed all 1,491 JavaScript lines and `package.json`, traced imports and build/test entry points across the repository, compared relevant Rust implementations, and exercised storage and helper behavior under both Node 24.19.0 and Bun 1.3.13.

**Verdict: Request changes before using this port for persistent application state.** The most serious defects are unsafe concurrent filesystem writes, oversized R2 deletion batches hidden by cleanup, and incorrect S3 signatures for encoded paths. Further findings concern conditional-write semantics, listing integrity, input validation, and compatibility.

**Reachability matters:** no production entry point in the reviewed tree imports these modules. The Makefile and release workflow build the Rust server and web application; `host/package.json` points to a nonexistent `core/index.js`. There is no JavaScript host server, CLI, or Worker entry point here. Consequently, this report identifies defects in the port, not demonstrated vulnerabilities in the running Rust deployment. Priorities below describe impact when these APIs are used. No P0 or presently reachable remote exploit was established.

Only this report was added. Existing and concurrent changes elsewhere in the working tree were not modified or attributed to this review.

## Critical issues / P1: address before integrating storage

### 1. Concurrent filesystem writes share one temporary file

**Location:** [host/adapters/fs.js:152](/home/vincent/repos/komodoc/host/adapters/fs.js:152), also `put()` at line 60.

Every write to a given key uses `${name}.tmp`. Two writes can open and truncate the same file, interleave their writes, and compete to rename it. A rename can expose a file that another writer still has open, defeating the stated guarantee that readers see only complete old or new bytes. Even identical content-addressed uploads can interfere.

**Reproduction:** run 20 concurrent `FsStore.put('index.json', ...)` calls with separate 100 KB JSON bodies using `Promise.allSettled`. Both Node and Bun produced **1 success and 19 failures**, with `ENOENT` renaming `index.json.tmp`. This requires only one store instance in one process; it does not depend on unsupported multiple processes. The observed failure is confirmed; partially mixed content is an additional possible interleaving, not an outcome asserted from this run.

**Fix:** give each write a uniquely named, exclusively created temporary file in the destination directory, rename only that writer's file, and clean it up on failure. Keep temporary files outside the logical listing namespace or explicitly exclude them. For example, use `open(uniqueName, 'wx')` followed by a write, close, and rename in a `try/finally`. Coordination for conditional writes is a separate requirement in finding 4.

**Regression:** simultaneous same-key writes must all complete according to the documented overwrite contract, and repeated reads must always contain exactly one complete submitted payload. Also exercise failed writes and temporary-file cleanup.

### 2. R2 cleanup exceeds the platform's deletion limit and silently leaves blobs behind

**Location:** [host/adapters/r2.js:35](/home/vincent/repos/komodoc/host/adapters/r2.js:35), [host/core/blob.js:101](/home/vincent/repos/komodoc/host/core/blob.js:101).

`R2Store.list()` gathers all pages, but `delete()` sends every key to one `bucket.delete()` call. R2 permits at most 1,000 keys per call. `clearStorage()` supplies the complete listing, catches the batch failure, then deletes `index.json` anyway. With 1,001 document objects, cleanup therefore resolves successfully after losing the index while the documents remain stored and billable. This threshold counts historical objects, not just active documents. The limit is documented in the [R2 Workers API reference](https://developers.cloudflare.com/r2/api/workers/workers-api-reference/#bucket-method-definitions).

**Reproduction:** an injected bucket returned pages of 1,000 and 1 objects and rejected delete batches larger than 1,000. `clearStorage(new R2Store(bucket))` submitted **1,001 keys**, swallowed the rejection, and deleted the index. This is a platform-contract mock, not a live R2 test.

**Fix:** chunk deletes, for example `for (let i = 0; i < keys.length; i += 1000) await bucket.delete(keys.slice(i, i + 1000).map(...))`. Return or aggregate cleanup failures so a reset cannot silently claim completion. The deliberate best-effort cleanup policy can remain, but callers need a partial-failure result.

**Regression:** 0, 1, 1,000, 1,001, and multiple thousands of keys; failure of a middle batch; ensure incomplete cleanup is reported.

### 3. S3 signs an already encoded path a second time

**Location:** [host/adapters/s3.js:79](/home/vincent/repos/komodoc/host/adapters/s3.js:79), also presigning at line 139 and URL construction at line 38.

`url()` encodes the key through `escapePath()`. `send()` then passes `new URL(target).pathname` through `escapePath()` again. URL pathnames retain percent escapes, so the signature covers a different path from the request. The same error occurs in `presignGet()`.

**Reproduction:** prefix `my project` and key `index.json` produce a wire path `/b/my%20project/index.json`, but a canonical path `/b/my%2520project/index.json`. An independent Node `createHmac`/`createHash` implementation matched this adapter's header and presigned signatures for prefix `plain`, and rejected both for `my project`, under both runtimes. Unicode, literal percent signs, and other escaped characters have the same problem. A configured prefix containing a space breaks even ordinary ASCII storage keys.

**Fix:** derive the transmitted and canonical paths from one correctly encoded representation. Avoid encoding `%` from an already encoded URL path. Preserve S3 key slashes and do not normalize object-key dot segments as part of a repair. The S3 rules are described in [AWS header signing](https://docs.aws.amazon.com/AmazonS3/latest/developerguide/sig-v4-header-based-auth.html) and [presigned URL signing](https://docs.aws.amazon.com/AmazonS3/latest/developerguide/sigv4-query-string-auth.html).

**Regression:** independent signature vectors for spaces, Unicode, `%`, `+`, and nested paths in both keys and configured prefixes; verify both signing modes against a real compatible service before release.

## Major issues / P2: correctness and conditional security risks

### 4. Filesystem compare-and-swap is not atomic relative to put or delete

**Location:** [host/adapters/fs.js:90](/home/vincent/repos/komodoc/host/adapters/fs.js:90), `put()` and `delete()` at lines 60 and 64.

Only `swap()` enters `serialize()`. An ordinary write or deletion can complete after the version read and before the conditional write. The swap still succeeds against the obsolete version. Serializing only swaps prevents two swaps from colliding, but does not provide the same conditional-write behavior as S3 or R2.

**Reproduction:** put `old`; begin a swap expecting its version; pause `readVersioned()` immediately after it has read the old object; delete the key and await completion; resume the swap. It succeeds and the final object is `new`. There is no valid atomic ordering: a swap before the delete should leave the object absent; a swap after it should conflict. The pause only selects an existing await boundary.

**Fix:** coordinate all same-key mutations through the same queue and make the swap call an internal unqueued write primitive to avoid recursive-queue deadlock. If the intended contract instead requires an external lock around all access, express and enforce that at the API boundary. The file's single-process assumption does not itself prevent this interleaving.

**Regression:** gated swap-versus-delete and swap-versus-put races, plus ordinary swap-versus-swap conflicts. Clarify whether separate `FsStore` instances over one directory are supported; their current queues are independent.

### 5. Filesystem containment does not hold across symbolic links

**Location:** [host/adapters/fs.js:22](/home/vincent/repos/komodoc/host/adapters/fs.js:22), reads at line 53 and writes at line 152.

`pathFor()` checks lexical containment only. A symlinked parent inside the data directory can point outside it; reads, writes, and deletes follow that parent. The predictable `.tmp` path can also follow a preexisting symlink when `writeFile()` truncates it.

**Reproduction:** create a temporary `store/link` symlink to a sibling `outside` directory. `get('link/value')` reads the outside file and `put('link/value', ...)` overwrites it. Both runtimes reproduced this with entirely disposable fixtures.

**Precondition:** somebody or some restore process must first place a symlink in the storage tree. No upload-to-symlink creation path was found. This is a containment defect under that condition, not a demonstrated remote traversal exploit, and the code explicitly treats keys as internally generated.

**Fix:** define whether symlinks are allowed. For a confined store, reject symlink components and use exclusive temporary-file creation. A `realpath()` check alone is insufficient against concurrent local replacement; robust hostile-local-writer protection needs directory-relative/no-follow primitives or an enforced private-directory ownership boundary.

**Regression:** intermediate and leaf symlinks, a preexisting temporary-file symlink, and paths through normal directories.

### 6. The S3 fallback version cannot be used as an S3 conditional ETag

**Location:** [host/adapters/s3.js:45](/home/vincent/repos/komodoc/host/adapters/s3.js:45), [host/adapters/s3.js:244](/home/vincent/repos/komodoc/host/adapters/s3.js:244).

When PUT omits `ETag`, `write()` invents a SHA-256 version. GET does not implement that fallback and can return `''`, which `swap()` interprets as create-only. If GET does return a provider ETag, there is still no guarantee it equals this SHA-256 string. Passing the invented value to `If-Match` will fail on a provider using a different ETag. The comment claiming that a later GET will report the same digest is not supported by the implementation.

**Reproduction:** a fetch mock returning successful PUT and GET responses without ETag produced a quoted SHA-256 version from `swap()` and an empty version from `getVersioned()` for identical bytes. A subsequent update takes the create-only branch. This affects the explicitly accommodated missing-ETag case, not normal responses carrying ETags.

**Fix:** conditional mode must obtain and retain an actual provider validator or fail clearly as unsupported. Do not repair this by merely hashing GET bodies too: a local digest still is not necessarily accepted by the service's `If-Match`. Single-writer mode may use a local informational digest under its explicitly weaker contract.

**Regression:** missing PUT ETag with present GET ETag, missing GET ETag, and an ETag unrelated to SHA-256; exercise `probe()` as well as repeated swaps.

### 7. S3 listing can report success with incomplete or corrupted results

**Location:** [host/adapters/s3.js:344](/home/vincent/repos/komodoc/host/adapters/s3.js:344), pagination at line 291.

The string parser does not validate the XML envelope or required fields. A cut-off `<Contents>` block is silently dropped, and a truncated listing without a next token is treated as finished. Separately, `NextContinuationToken` is not XML-decoded, while the key decoder handles only five named entities and leaves numeric character references unchanged. These yield false empty/partial listings, requests with the wrong opaque continuation token, or deletion attempts against the wrong key.

**Reproductions:**

- `parseListing('<ListBucketResult><Contents><Key>documents/a</Key>')` returns a successful empty listing.
- `<NextContinuationToken>a&amp;b</NextContinuationToken>` yields `a&amp;b`, not `a&b`.
- `<Key>a&#13;b</Key>` yields the literal entity spelling, not the original key.

The code intentionally avoids DOMParser for Worker compatibility; that is reasonable, but a parser still needs to reject malformed input. AWS explicitly cautions that even HTTP 200 can contain invalid XML in [ListObjectsV2](https://docs.aws.amazon.com/AmazonS3/latest/API/API_ListObjectsV2.html).

**Fix:** use a Worker-compatible XML parser, or implement a validated subset with complete entity decoding and required-field checks. Reject missing or repeated continuation tokens on truncated pages. Keep malformed-response errors distinct from a legitimate empty bucket. Consider `encoding-type=url` with the corresponding key decoding.

**Regression:** malformed XML, missing fields, escaped tokens, numeric entities, empty valid lists, and multiple pages. Verify incomplete results cannot be consumed as a successful cleanup or inventory.

### 8. MemoryStore aliases Node/Bun Buffers and invalidates its own versions

**Location:** [host/adapters/memory.js:18](/home/vincent/repos/komodoc/host/adapters/memory.js:18), lines 26 and 31 as well.

The adapter uses `.slice()` to copy both incoming and outgoing bytes. That copies a plain `Uint8Array`, but a `Buffer` is a `Uint8Array` subclass whose `.slice()` shares storage. A caller can change a stored object after `put()`, or mutate the result of `get()`, without a write or a version update. This is especially misleading in the advertised reference adapter: tests using plain arrays will miss behavior caused by filesystem Buffer inputs.

**Reproduction:** store `Buffer.from('old')`, modify the original buffer, then modify the result of `getVersioned()`. The final stored body becomes `ned`, while the version remains the digest of `old`. Confirmed on Node and Bun.

**Fix:** explicitly copy into an owned base typed array, for example `const owned = new Uint8Array(body)`, and hash that same owned snapshot. Return owned copies too.

**Regression:** both Buffer and Uint8Array inputs; mutation of input and read results; assert `version === await versionOf(returnedBody)` throughout.

### 9. Rule setters accept invalid numeric values and partially apply rejected settings

**Location:** [host/core/rules.js:130](/home/vincent/repos/komodoc/host/core/rules.js:130), `setStorage()` at line 139 and `setCounts()` at line 154.

Relational comparisons are not numeric validation in JavaScript. `setMaxDocument(rules, 'oops')` reports success and installs `NaN`; ordinary `size > max_document` checks then always fail to reject. `setCounts(rules, Infinity, 0)` installs an infinite count. `NaN` can also be accepted as a no-op through truthiness checks. Unlike the typed Rust inputs, these public functions have no type boundary here.

In addition, `setStorage()` mutates limits before checking the relationship between them. `setStorage(rules, 6000, 0)` returns an error but leaves `per_owner` above `total`. Its `>> 20` formatting wraps through a signed 32-bit conversion: the message reports **1904 MB versus 1024 MB**, although the values are 6000 MB versus the default 5120 MB.

**Precondition:** invalid configuration reaches these exported functions; no host CLI exists here to establish prior validation.

**Fix:** require finite numeric inputs, require integers where counts demand them, compute prospective values locally, validate the complete result, then commit it. For example, `typeof n === 'number' && Number.isFinite(n)` and `Number.isInteger(count)`. Format MB with division rather than bitwise shifts.

**Regression:** strings, NaN, infinities, negative values, fractional counts, zero-as-default, valid boundaries, and equality of the rules object before and after every rejected call.

### 10. Cleanup omits storage namespaces created by the current application

**Location:** [host/core/blob.js:102](/home/vincent/repos/komodoc/host/core/blob.js:102); compare [crates/komodoc/src/blob.rs:380](/home/vincent/repos/komodoc/crates/komodoc/src/blob.rs:380).

The JavaScript reset deletes only `documents/`, `sources/`, `rooms/`, and `examples/`. The current Rust application also stores `sessions/`, `history/`, `assets/`, and `renderings/`, and its cleanup includes those prefixes. Running this port's cleanup over an existing application bucket removes its index while retaining those objects. This is an actual operation mismatch, not just a stale comment.

**Reproduction:** seed a MemoryStore with one object per namespace plus the index; call `clearStorage()`. The four newer namespaces remain. `session.key` is a different object and is intentionally retained by both implementations.

**Precondition:** the port is used to reset an existing/current-layout store. If compatibility is intentionally out of scope, explicitly reject that layout instead of appearing to clear it.

**Fix:** bring the owned namespace inventory into agreement with the current key layout and centralize it so new stored object types cannot be omitted from reset. Retain unrelated bucket objects and the intended signing key.

**Regression:** a complete current-layout fixture, reserved examples, unrelated prefixes, and `session.key` preservation.

### 11. Accepted document paths are not safe to materialize on Windows

**Location:** [host/core/rules.js:188](/home/vincent/repos/komodoc/host/core/rules.js:188), collision handling at line 176.

`checkPath()` rejects POSIX traversal and backslashes but accepts `C:/paper.tex`, `CON.tex`, and `dir./a.tex`. These include drive syntax, a reserved device name, and a directory component that can normalize differently on Windows. `collisionKey()` only normalizes Unicode and lowercase, so `dir./a.tex` and `dir/a.tex` remain different keys even where local filesystem normalization collapses them.

**Reproduction:** all three paths return `{ kind: 'text' }` under the default rules. Windows reserves device names even with extensions and restricts colons and trailing dots/spaces, as documented in [Microsoft's filename rules](https://learn.microsoft.com/en-us/windows/win32/fileio/naming-a-file).

**Precondition:** an accepted document is later written to Windows by a sync/export client. No JavaScript host sync writer exists in this tree, so this is a portability/data-integrity defect at that integration boundary, not a demonstrated arbitrary-file-write exploit.

**Fix:** define a portable per-segment path policy including drive syntax, device names, forbidden characters, and trailing-dot/space normalization. Align host admission and client materialization, with collision detection using that same policy.

**Regression:** Windows-reserved names with extensions and nested paths, trailing spaces/dots on directory components, and pairs that would alias during materialization. Test on Windows before claiming the full guarantee.

## Minor issues / P3

### 12. Origin construction is inconsistent about hostname case and default ports

**Location:** [host/core/origins.js:46](/home/vincent/repos/komodoc/host/core/origins.js:46), lines 72 and 90.

`isDocsHost()` recognizes `DOCS.example.com`, but `readerHost()` does not remove its uppercase prefix. An arrival of `EXAMPLE.COM:443` over HTTPS produces an origin unequal to the browser's canonical `https://example.com`; a valid same-origin mutation with its client marker is rejected. Both cases were reproduced by direct calls.

Normalize the hostname and serialize origins through `URL`, including default-port handling, before routing or equality comparisons. Keep tests for foreign document origins; a normalization repair must not weaken their refusal. This is a false-refusal/routing defect; no bypass was demonstrated. It is inherited from the corresponding Rust logic rather than newly introduced by JavaScript.

### 13. `unhex()` accepts malformed byte pairs

**Location:** [host/core/util.js:64](/home/vincent/repos/komodoc/host/core/util.js:64).

`Number.parseInt()` accepts a valid prefix: `unhex('1g')` returns `[1]` instead of rejecting the malformed encoding. Validate the entire even-length string before conversion, for example `/^(?:[0-9a-fA-F]{2})*$/`. Add invalid-nibble tests in both positions. No production caller currently imports this helper, so this is not evidence of an authentication bypass.

### 14. Text-boundary utilities lose or retain characters incorrectly

**Location:** [host/core/util.js:32](/home/vincent/repos/komodoc/host/core/util.js:32), [host/core/util.js:89](/home/vincent/repos/komodoc/host/core/util.js:89).

Two small confirmed edge cases:

- `clean('abc', 0)` returns `a`, because the limit is checked after appending. Return early for a nonpositive limit or check before appending. The Rust `.take(0)` behavior returns empty.
- `truncate('�a', 3)` returns empty, although the first character is a complete three-byte U+FFFD that fits. Removing a trailing replacement character cannot distinguish original content from a decoder repair. Find a valid byte boundary before decoding instead.

Add zero-limit tests and byte limits immediately before, within, and after multibyte characters, including an actual U+FFFD. Current S3 error formatting is a caller of `truncate()`, so the latter can silently lose error-message content.

### 15. Timestamp parsing validates shape but normalizes invalid calendar values

**Location:** [host/core/clock.js:34](/home/vincent/repos/komodoc/host/core/clock.js:34).

`parseTimestamp('2026-02-30T00:00:00Z')` returns March 2's epoch seconds, and `2026-01-01T24:00:00Z` becomes the next day. Both are accepted by `Date.parse()` despite the helper promising strict RFC 3339 validation. This changes how malformed persisted lock timestamps are classified.

Validate calendar and time components, including day-of-month/leap-year boundaries, before conversion. Add valid offset/fraction tests alongside invalid dates and hour 24. No live retention-sweep caller exists in `host/`; the concrete in-tree caller is `takeRoomLock()`.

### 16. The exported frozen rules object is mutable below the first level

**Location:** [host/core/rules.js:112](/home/vincent/repos/komodoc/host/core/rules.js:112).

`Object.freeze(defaultRules())` freezes only the outer object. `RULES.storage.total = 1` succeeds, as do edits to nested caps and extension arrays, changing the singleton seen by other importers. I confirmed and then restored the value within the disposable test process. This has no current production importer, but violates the shared immutable-default contract.

Deep-freeze the exported singleton, or export fresh defaults to consumers instead. Keep `defaultRules()` mutable for deployment-specific settings. Test nested objects and arrays, not just `Object.isFrozen(RULES)`.

## Integration concerns and questions for the author

These are intentionally separate from confirmed defect priorities because the surrounding host implementation is absent.

- **Is this an active port or retained scaffolding?** `package.json` advertises a host/server/CLI and names missing `core/index.js`. No checked-in host test suite, server, Worker, or CLI establishes how the APIs are meant to be called. Decide its lifecycle before treating unused exports as production requirements or deleting them.
- **What enforces the single-writer assumption?** The filesystem comments assume process ownership; the S3 `singleWriter` option deliberately skips conditional writes. Neither adapter enforces an external lock. `takeRoomLock()` explicitly returns `{ mine: true }` on a storage read/write error; I reproduced the read-error case. Its comments say it is legacy compatibility, so I have not counted that deliberate behavior as a live locking bug. Do not use it as a distributed lease without failure refusal, renewal, fencing, and ownership-aware release. A generic Worker binding itself is not evidence of one authoritative room owner; identify the intended Durable Object or equivalent boundary.
- **Which headers are trusted at ingress?** `Arrival.fromHeaders()` derives everything from Host and X-Forwarded-Proto. The eventual runtime/proxy must supply trustworthy arrival metadata. The bearer-prefix branch is a CSRF exemption, not token validation; the authentication layer must still reject invalid bearer credentials. No authentication or proxy configuration in `host/` exists to review for that connection.
- **Should R2 and S3 normalize prefixes identically?** S3 adds a trailing slash to `prefix: 'tenant'`; R2 concatenates it verbatim. Thus the same prefix input addresses different keys. Either normalize in both constructors or require an explicit already-normalized prefix at a common boundary.
- **S3 signing helpers have another latent edge:** `canonicalQuery()` sorts unencoded strings. For repeated `prefix` values `z` and `é`, it emits `prefix=z&prefix=%C3%A9`, whereas encoded sorting places `%C3%A9` first. Current generated requests use distinct ASCII parameter names, so this is not a demonstrated failure of their normal query path. Correct encoded-pair sorting if the public `send()`/helper surface is retained.
- **Filesystem listing scales with the whole store.** `walk()` descends into every directory before applying the requested prefix. A per-document listing therefore visits unrelated document trees, and four reset prefixes repeat the traversal. No live host caller establishes an N+1 request pattern, so this is a measured-from-code complexity concern, not a claimed benchmark regression. Prune traversal by compatible prefixes if this adapter becomes active.
- **Concurrent S3 probes share `.komodoc-probe`.** Each probe writes and deletes that same object. Concurrent startups can invalidate each other's capability checks. Use unique probe keys if simultaneous startup against one prefix is supported; this review did not run a live multi-client probe.

## Coverage and verification

`bun test` in `host/` exits **1** with **“No tests found!”**. `host/package.json` has only that test script. The root Makefile, CI, release build, and web checks do not execute host modules. Rust tests exercise Rust code and cannot establish correctness of this port. Comments claiming that a shared adapter suite runs over all implementations are not backed by checked-in JavaScript tests.

I ran disposable review scripts under both Node and Bun. They performed real filesystem operations only in fresh `/tmp/host-*` fixtures, injected fetch/R2 responses, exercised all four adapters, compared signatures against a separate implementation, and called rules/origin/clock/byte helpers. They confirmed the outputs quoted above. Positive assertions verified same-origin acceptance, document-origin HTTP and WebSocket refusal, missing-marker refusal, exactly one winner for racing MemoryStore create-only swaps, and queue recovery after a conflict. Source modules were not changed to obtain these results.

**Limits:** no live AWS, R2, MinIO, Worker runtime, Windows filesystem, browser, or production deployment was used. The full Rust/web suite was not rerun because this review changes no implementation and those suites do not cover `host/`. Storage service claims were checked against the primary documentation linked above. The reproduction programs are preserved below so evidence does not depend on ephemeral `/tmp` files.

Recommended first test investment: a common adapter contract suite for byte ownership, not-found handling, create-only/update conflicts, all mutation interleavings, prefix isolation, pagination, and bulk deletion. Then add independent S3 signing fixtures and a small service integration suite. Bind the host suite into CI if this port remains active.

## Positive feedback

- Storage concerns are separated cleanly from runtime-neutral code; WebCrypto keeps the signing implementation portable.
- `getOrNull()` preserves storage errors rather than treating every failure as absence.
- Versioned source and document keys support writing content before committing the index, avoiding overwriting the source of a currently published version.
- The serialization queue deliberately absorbs previous rejection, and the positive check confirmed later swaps still run after a conflict.
- R2 listing follows the platform's `truncated` flag rather than incorrectly assuming that a short page is the last page.
- Origin checks account for the fact that document subdomains are same-site with the reader. The tested foreign-origin refusals behaved correctly.
- `defaultRules()` returns independent nested settings; the singleton-freezing issue does not invalidate that useful separation.

## Reproduction programs

The scripts below are diagnostic programs that print observed behavior, not a committed regression suite asserting that bugs should persist. Run from any directory after replacing `/home/vincent/repos/komodoc` if the checkout is elsewhere. They use fake S3 credentials and injected responses; they never contact a bucket. The filesystem script leaves its uniquely named temporary fixtures available for inspection. JSON renders NaN and Infinity as `null` in its numeric-rule output.

### A. Adapter and helper diagnostics

```js
import assert from 'node:assert/strict';
import {mkdtemp, readFile, writeFile, mkdir, symlink} from 'node:fs/promises';
import {createHash, createHmac} from 'node:crypto';
import {FsStore} from '/home/vincent/repos/komodoc/host/adapters/fs.js';
import {MemoryStore} from '/home/vincent/repos/komodoc/host/adapters/memory.js';
import {R2Store} from '/home/vincent/repos/komodoc/host/adapters/r2.js';
import {S3Store, escapePath, parseListing, canonicalQuery} from '/home/vincent/repos/komodoc/host/adapters/s3.js';
import {clearStorage, takeRoomLock, BlobError, versionOf} from '/home/vincent/repos/komodoc/host/core/blob.js';
import {parseTimestamp, setClock} from '/home/vincent/repos/komodoc/host/core/clock.js';
import {utf8, fromUtf8, unhex, clean, truncate} from '/home/vincent/repos/komodoc/host/core/util.js';
import {RULES, defaultRules, setMaxDocument, setStorage, setCounts, checkPath} from '/home/vincent/repos/komodoc/host/core/rules.js';
import {Arrival,crossSiteRefused} from '/home/vincent/repos/komodoc/host/core/origins.js';
const results=[];
const check=async(name,fn)=>{try {results.push({name,result:await fn()})}catch(e){results.push({name,error:e.stack})}};
await check('same-key parallel filesystem puts',async()=>{
 const dir=await mkdtemp('/tmp/host-race-'); const s=new FsStore(dir);
 const writes=await Promise.allSettled(Array.from({length:20},(_,i)=>s.put('index.json',utf8(JSON.stringify({i,data:'x'.repeat(100000)})))));
 return {fulfilled:writes.filter(x=>x.status==='fulfilled').length,rejected:writes.filter(x=>x.status==='rejected').length,firstError:writes.find(x=>x.status==='rejected')?.reason.message};
});
await check('swap overtaken by delete',async()=>{
 const s=new FsStore(await mkdtemp('/tmp/host-cas-')); await s.put('index.json',utf8('old')); const version=(await s.getVersioned('index.json'))[1];
 const original=s.readVersioned.bind(s); let entered,release;
 const enteredP=new Promise(r=>entered=r); const gate=new Promise(r=>release=r);
 s.readVersioned=async key=>{const value=await original(key); entered(); await gate; return value};
 const pending=s.swap('index.json',utf8('new'),version); await enteredP; await s.delete(['index.json']); release(); await pending;
 return fromUtf8((await original('index.json'))[0]);
});
await check('filesystem symlink escape',async()=>{
 const dir=await mkdtemp('/tmp/host-links-'); await mkdir(`${dir}/store`);await mkdir(`${dir}/outside`);await writeFile(`${dir}/outside/value`,'outside');await symlink(`${dir}/outside`,`${dir}/store/link`);
 const s=new FsStore(`${dir}/store`);const before=fromUtf8(await s.get('link/value'));await s.put('link/value',utf8('overwritten'));return {before,after:await readFile(`${dir}/outside/value`,'utf8')};
});
await check('memory Buffer ownership',async()=>{const s=new MemoryStore();const body=Buffer.from('old');await s.put('x',body);body[0]=110;const [read,version]=await s.getVersioned('x');read[1]=101;return {stored:fromUtf8(await s.get('x')),versionMatches:version===await versionOf(await s.get('x'))}});
await check('R2 bulk clear',async()=>{
 let indexDeleted=false,batch=0; const bucket={list:async ({prefix})=>({objects:prefix==='documents/'?Array.from({length:1000},(_,i)=>({key:`documents/${i}`,size:1,etag:'e'})):[],truncated:prefix==='documents/',cursor:'next'}),delete:async keys=>{batch=Math.max(batch,keys.length);if(keys.length>1000)throw Error('1000 key limit');if(keys.includes('index.json'))indexDeleted=true}};
 bucket.list=async ({prefix,cursor})=>({objects:prefix==='documents/'?Array.from({length:cursor?1:1000},(_,i)=>({key:`documents/${cursor?1000:i}`,size:1,etag:'e'})):[],truncated:prefix==='documents/'&&!cursor,cursor:'next'});
 await clearStorage(new R2Store(bucket));return {batch,indexDeleted};
});
await check('S3 escaped paths',async()=>{const s=new S3Store({endpoint:'https://example.com',bucket:'b',prefix:'my project',accessKey:'fake',secretKey:'fake'});const path=new URL(s.url('index.json')).pathname;return {wire:path,signed:escapePath(path)}});
await check('S3 version fallback',async()=>{const s=new S3Store({endpoint:'https://example.com',bucket:'b',accessKey:'fake',secretKey:'fake'},async(url,init)=>new Response(init.method==='GET'?'hello':null,{status:200}));return {writeVersion:await s.swap('x',utf8('hello'),''),readVersion:(await s.getVersioned('x'))[1]}});
await check('listing XML and query',async()=>({token:parseListing('<IsTruncated>true</IsTruncated><NextContinuationToken>a&amp;b</NextContinuationToken><Contents><Key>a&#13;b</Key></Contents>').nextToken,key:parseListing('<Contents><Key>a&#13;b</Key></Contents>').contents[0][0],query:canonicalQuery([['prefix','z'],['prefix','é']])}));
await check('lock errors treated as ownership',async()=>takeRoomLock({getVersioned:async()=>{throw BlobError.other('offline')}},'paper','server'));
await check('clear legacy/current layout',async()=>{const s=new MemoryStore();for(const key of ['documents/a/b.html','sources/a/b','sessions/a/b','history/a/index.json','assets/a/b','renderings/a/b','index.json'])await s.put(key,utf8('x'));await clearStorage(s);return (await s.list('')).map(x=>x.key)});
await check('timestamp validation',async()=>({feb30:parseTimestamp('2026-02-30T00:00:00Z'),hour24:parseTimestamp('2026-01-01T24:00:00Z')}));
await check('numeric rules',async()=>{const r=defaultRules();return {message:setMaxDocument(r,'oops'),max_document:r.max_document,countsMessage:setCounts(r,Infinity,0),count:r.storage.documents_per_owner,storageMessage:setStorage(r,6000,0),per_owner:r.storage.per_owner}});
await check('utilities',async()=>({unhexBad:Array.from(unhex('1g')),zeroClean:clean('abc',0),control:clean('a\u0085b',9),replacement:truncate('�a',3)}));
await check('origin normalization',async()=>{const a=new Arrival('EXAMPLE.COM:443','https');return {refused:crossSiteRefused(new Headers({origin:'https://example.com','x-komodoc-client':'web','sec-fetch-site':'same-origin'}),a),docsReader:new Arrival('DOCS.example.com','https').readerOrigin()}});
await check('path checks',async()=>['C:/paper.tex','CON.tex','dir./a.tex','a\u0085.tex'].map(path=>({path,...checkPath(defaultRules(),path)})));
console.log(JSON.stringify(results,null,2));
```

### B. Independent signatures and positive assertions

```js
import assert from 'node:assert/strict';
import {createHash,createHmac} from 'node:crypto';
import {MemoryStore} from '/home/vincent/repos/komodoc/host/adapters/memory.js';
import {S3Store,parseListing} from '/home/vincent/repos/komodoc/host/adapters/s3.js';
import {setClock} from '/home/vincent/repos/komodoc/host/core/clock.js';
import {RULES,defaultRules,validSlug} from '/home/vincent/repos/komodoc/host/core/rules.js';
import {Arrival,crossSiteRefused,wsOriginRefused} from '/home/vincent/repos/komodoc/host/core/origins.js';
import {utf8} from '/home/vincent/repos/komodoc/host/core/util.js';
setClock(()=>Date.parse('2026-09-05T12:00:00Z')/1000);
const hash=s=>createHash('sha256').update(s).digest('hex');const hmac=(k,s)=>createHmac('sha256',k).update(s).digest();
function signature(path,headers,query='',presigned=false){const names=presigned?['host']:['host','x-amz-content-sha256','x-amz-date'];const scope='20260905/auto/s3/aws4_request'; const canonical=['GET',path,query,names.map(k=>`${k}:${k==='host'?'example.com':headers.get(k)}\n`).join(''),names.join(';'),presigned?'UNSIGNED-PAYLOAD':hash('')].join('\n');let key=hmac('AWS4fake','20260905');for(const part of ['auto','s3','aws4_request'])key=hmac(key,part);return hmac(key,`AWS4-HMAC-SHA256\n20260905T120000Z\n${scope}\n${hash(canonical)}`).toString('hex')}
for(const prefix of ['plain','my project']) {let call;const s=new S3Store({endpoint:'https://example.com',bucket:'b',prefix,accessKey:'fake',secretKey:'fake'},async(url,init)=>{call={url,init};return new Response('x',{headers:{etag:'"e"'}})});await s.get('index.json');const actual=call.init.headers.get('authorization').split('Signature=')[1];const correct=signature(new URL(call.url).pathname,call.init.headers);const url=new URL(await s.presignedGet('index.json',60));const actualPresigned=url.searchParams.get('X-Amz-Signature');const query=url.search.slice(1).split('&').filter(x=>!x.startsWith('X-Amz-Signature=')).join('&'); console.log('signature',prefix,{headerMatches:actual===correct,presignedMatches:actualPresigned===signature(url.pathname,null,query,true)});}
console.log('newline slug',JSON.stringify(['paper\n','paper\r','paper\u2028','paper'].map(s=>[s,validSlug(defaultRules(),s)])));
console.log('invalid XML',parseListing('<ListBucketResult><Contents><Key>documents/a</Key>'));
const saved=RULES.storage.total;RULES.storage.total=1;console.log('frozen rules mutated',RULES.storage.total===1);RULES.storage.total=saved;
const a=new Arrival('example.com','https');assert.equal(crossSiteRefused(new Headers({origin:'https://docs.example.com','x-komodoc-client':'web'}),a),true);assert.equal(crossSiteRefused(new Headers({origin:'https://example.com','sec-fetch-site':'same-origin','x-komodoc-client':'web'}),a),false);assert.equal(crossSiteRefused(new Headers(),a),true);assert.equal(wsOriginRefused(new Headers({origin:'https://docs.example.com'}),a),true);
const s=new MemoryStore();const outcomes=await Promise.allSettled([s.swap('x',utf8('a'),''),s.swap('x',utf8('b'),'')]);assert.equal(outcomes.filter(x=>x.status==='fulfilled').length,1);assert.equal(outcomes.find(x=>x.status==='rejected').reason.kind,'conflict');const [body,v]=await s.getVersioned('x');await s.swap('x',body,v);console.log('positive origin and CAS checks passed');
```

## Review process note

The code-reviewer skill needed no changes; this review did not reveal a generalizable problem in its instructions.
