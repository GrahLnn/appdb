# appdb load and churn performance evidence

The performance test is [core/tests/perf_load.rs](C:/Users/admin/appdb/core/tests/perf_load.rs). It uses the real public Rust API and the application `init_local_app_db` policy. Each phase runs in a fresh child process because SurrealDB documents that an embedded datastore directory may be opened by only one process, and a second open in the same process also fails while the first client owns the lock. This makes the read phases real cold process restarts instead of measuring an invalid in-process reopen. Reported timings below are from the optimized release test binary; debug runs were used only to debug the harness. The host reported an Intel Core Ultra 9 285K (24 cores), 127.3 GiB RAM, and NVMe SSD storage; the database paths were on `C:`.

The SQLite comparison uses Python 3.14's native `sqlite3` binding with SQLite
3.50, `journal_mode=DELETE`, `synchronous=FULL`, one table, one primary-key
index, and the same `(sequence, id)` pagination index and 256-byte payload. Each
phase is a fresh Python child process. SQLite statements use prepared
`executemany` calls inside one transaction; appdb measurements use the public
Rust model/repository API. This is therefore an application-path comparison,
not a claim that Python and Rust driver overhead are identical. The reproducible
script is `rounds/2026-10-08-appdb-refactor/sqlite_perf.py`.

The test phases are:

1. Seed the database with `N` rows.
2. Exit and reopen, then measure keyset page, single-record get, bounded list, and full list.
3. Exit and reopen, update every row, delete `25%` per round, insert the same number, and repeat the requested number of rounds.
4. Exit and reopen again, then repeat the read measurements.

## Measurements

### 20,000 rows, 3 churn rounds, 5,000 deletes/inserts per round (release)

Command:

```powershell
$env:APPDB_PERF_ROWS='20000'
$env:APPDB_PERF_DELETE_ROWS='5000'
$env:APPDB_PERF_CHURN_ROUNDS='3'
cargo test -p appdb --test perf_load --release --no-default-features -- --ignored --nocapture
```

Observed metric after direct keyset pagination and schema fingerprinting:

```text
seed:       open 613.147 ms, write 633.036 ms, 16,249,347 bytes, 4 files
baseline:   open 583.985 ms, page 22.132 ms, get 0.377 ms, list_limit 0.402 ms, list_all 52.315 ms, 16,249,423 bytes
churn:      open 525.329 ms, load_all 77.419 ms, churn 35,950.580 ms, 59,872,150 bytes
post-churn: open 718.525 ms, page 24.831 ms, get 0.389 ms, list_limit 6.241 ms, list_all 58.528 ms, 59,872,316 bytes
```

### 100,000 rows, 1 churn round, 25,000 deletes/inserts (release)

Command:

```powershell
$env:APPDB_PERF_ROWS='100000'
$env:APPDB_PERF_DELETE_ROWS='25000'
$env:APPDB_PERF_CHURN_ROUNDS='1'
cargo test -p appdb --test perf_load --release --no-default-features -- --ignored --nocapture
```

Observed metric after direct keyset pagination and schema fingerprinting:

```text
seed:       open 589.469 ms, write 3153.772 ms, 81,373,826 bytes, 4 files
baseline:   open 944.326 ms, page 22.273 ms, get 0.306 ms, list_limit 0.338 ms, list_all 267.701 ms, 81,373,902 bytes
churn:      open 852.441 ms, load_all 276.963 ms, churn 59,563.633 ms, 154,430,995 bytes
post-churn: open 1119.402 ms, page 8.039 ms, get 0.251 ms, list_limit 6.675 ms, list_all 288.308 ms, 154,431,161 bytes
```

The 100k release run completed successfully in 67.40 seconds. The four files
include the schema fingerprint sidecar. After one churn round, storage bytes
were about 1.90x the baseline, while the cold reopen rose from 0.944 s to
1.119 s. Single-record get stayed below 0.4 ms. These are single samples and
are cache-sensitive, so they are directional rather than a statistical
benchmark.

The optional `APPDB_PERF_DIAGNOSE=1` release run checks the actual plan. The
pagination field uses an `IndexScan`; on the 2k diagnostic run the direct
bounded query took 2.077 ms and the production keyset path, including decode,
took 20.926 ms. Replaying all generated `DEFINE INDEX OVERWRITE` statements
took 84.915 ms and grew the store by about 0.31 MB; that operation remains
available for an explicit schema change, while normal Rust startup skips it
when the sidecar fingerprint matches.

Diagnostic command:

```powershell
$env:APPDB_PERF_ROWS='2000'
$env:APPDB_PERF_DELETE_ROWS='500'
$env:APPDB_PERF_CHURN_ROUNDS='1'
$env:APPDB_PERF_DIAGNOSE='1'
cargo test -p appdb --test perf_load --release --no-default-features -- --ignored --nocapture
```

## What the results establish

- The startup path is lazy when it uses `pagin_asc` or a bounded list: the first page and single-record reads do not load all rows. The full `list()` path is intentionally still an eager path and should not be used during desktop startup for large tables; it is measured here to make that cost visible.
- The 20k run reduced baseline cold open from 1.099 s to 0.584 s and the first page from 83.2 ms to 22.1 ms. After three churn rounds, open was 0.719 s and the first page 24.8 ms.
- The 100k run reduced baseline cold open from 3.585 s to 0.944 s and the first page from 433.8 ms to 22.3 ms. After one churn round, open was 1.119 s and the first page 8.0 ms.
- The prior relation and graph batching changes remain usable. All four ignored relation/graph smoke tests passed. The measured averages were about 2.76 s for the 12-root batch relation replacement, 604 ms for a single 64-item relation replacement, and 1.9–6.7 ms for the batched outgoing/incoming graph accessors.
- The previous structural fixes all remain green: Rust integration CRUD tests 126/126, workspace Rust tests, locked all-target check, TypeScript typecheck, 88 TypeScript tests, TypeScript build, and Bend2 proof checks.
- The test does not claim that physical fragmentation is mathematically absent. It is a public black-box proxy: storage bytes, file count, cold reopen, paged reads, bounded reads, and full reads before and after real CRUD churn. The remaining 1.90x byte growth after 100k churn is write amplification from the LSM storage path; SQLite retained a flat file in the same comparison.

## Official usage check

The current usage follows the public Rust SDK surface: `SurrealKv`, `Surreal`, typed `SurrealValue` models, startup schema application, and one opened datastore handle shared by the application. SurrealDB's embedding guide documents SurrealKV as the Rust embedded file engine and says the datastore directory is single-open; the SDK has no synchronous `close()` method, so process-boundary restart is the valid cold-start measurement. The SDK documents `begin()`/`commit()` for multi-statement transactions, and the appdb write paths keep parent and relation writes in one transaction plan where the public API permits it. The app uses keyset pagination for startup-scale reads and reserves full-table listing for explicit callers.

The usage is public-API-correct for this embedded desktop path: schema work is
fingerprint-gated, pagination is a bounded keyset scan, and full-table reads are
explicit. The extra sidecar file is metadata, not a second data store; its
presence explains the measured four-file count.

## SQLite comparison

The same workload produced these SQLite measurements:

```text
20k SQLite:  baseline open 1.643 ms, page 0.161 ms, get 0.058 ms,
            list_limit 0.099 ms, list_all 20.607 ms, 7,094,272 bytes
            post-churn open 2.000 ms, page 0.351 ms, get 0.060 ms,
            list_limit 0.378 ms, list_all 25.167 ms, 7,094,272 bytes
            churn 145.220 ms

100k SQLite: baseline open 1.283 ms, page 0.228 ms, get 0.097 ms,
            list_limit 0.096 ms, list_all 97.390 ms, 35,651,584 bytes
            post-churn open 1.205 ms, page 0.150 ms, get 0.055 ms,
            list_limit 0.092 ms, list_all 91.316 ms, 35,651,584 bytes
            churn 411.352 ms
```

Against the appdb application path, SQLite is roughly 3–7x faster for point
and baseline bounded reads; post-churn bounded reads were up to about 16x
faster. Full reads were 2–3x faster, and measured churn was more than two
orders of magnitude faster. Its cold reopen is around 1–2 ms,
while appdb is 0.584–1.119 s. SQLite also kept the file at 7.09 MB for 20k and
35.65 MB for 100k after churn; appdb grew from 16.25 to 59.87 MB and from
81.37 to 154.43 MB respectively.

The cold-open comparison is robust directionally because it measures separate
processes and full durability settings. Exact query multipliers are less
strict: appdb includes Rust model encoding, repository planning, and hydration,
whereas SQLite uses direct prepared SQL. A fair storage-engine-only comparison
requires adding a raw-Surreal query suite alongside the application-path suite;
the existing 2k diagnostic already showed the direct appdb page query at
2.077 ms versus 20.926 ms through the production pagination/hydration path.

SurrealDB labels SurrealKV as beta and recommends RocksDB for conservative critical single-node production workloads. For this desktop local-first use case, SurrealKV remains the selected embedded engine, but the measured write amplification means a repeated-churn maintenance/compaction strategy should be evaluated before calling the storage path fragmentation-free.
