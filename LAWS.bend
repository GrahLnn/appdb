import Base
import ./appdb_model.bend as Model
import ./benchmark_model.bend as Bench
import ./endpoint_model.bend as Endpoint

law close_idempotent:
  for x: Model.Lifecycle
  {Model.close(Model.close(x)) == Model.close(x) : Model.Lifecycle}

law commit_idempotent:
  for x: Model.WritePlan
  {Model.commit(Model.commit(x)) == Model.commit(x) : Model.WritePlan}

law reuse_free_slot:
  for live: Nat
  for tail: Nat
  {Model.remove(Model.insert(Model.Layout{live, 1n+tail})) ==
    Model.Layout{live, 1n+tail} : Model.Layout}

law same_key_last_write:
  for entries: Bench.Entries
  for first: Nat
  for last: Nat
  {Bench.put_left(Bench.put_left(entries, first), last) ==
    Bench.put_left(entries, last) : Bench.Entries}

law delete_key_idempotent:
  for entries: Bench.Entries
  {Bench.delete_left(Bench.delete_left(entries)) ==
    Bench.delete_left(entries) : Bench.Entries}

law distinct_key_writes_commute:
  for entries: Bench.Entries
  for left: Nat
  for right: Nat
  {Bench.put_left(Bench.put_right(entries, right), left) ==
    Bench.put_right(Bench.put_left(entries, left), right) : Bench.Entries}

law distinct_key_delete_write_commute:
  for entries: Bench.Entries
  for right: Nat
  {Bench.delete_left(Bench.put_right(entries, right)) ==
    Bench.put_right(Bench.delete_left(entries), right) : Bench.Entries}

law native_path_roundtrip:
  for backend: Nat
  for path: Endpoint.NativePath
  {Endpoint.path(Endpoint.encode(backend, path)) == path : Endpoint.NativePath}
