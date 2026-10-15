import Base
import ./LAWS.bend as Laws
import ./appdb_model.bend as Model
import ./benchmark_model.bend as Bench

def Laws.close_idempotent(x):
  match x:
    case Model.Open{}:
      {==}
    case Model.Closed{}:
      {==}

def Laws.commit_idempotent(x):
  match x:
    case Model.Planned{writes}:
      {==}
    case Model.Committed{writes}:
      {==}
    case Model.Aborted{}:
      {==}

def Laws.reuse_free_slot(live, tail):
  {==}

def Laws.same_key_last_write(entries, first, last):
  match entries:
    case Bench.Entries{left, right}:
      {==}

def Laws.delete_key_idempotent(entries):
  match entries:
    case Bench.Entries{left, right}:
      {==}

def Laws.distinct_key_writes_commute(entries, left, right):
  match entries:
    case Bench.Entries{a, b}:
      {==}

def Laws.distinct_key_delete_write_commute(entries, right):
  match entries:
    case Bench.Entries{a, b}:
      {==}

def main() -> {Model.remove(Model.insert(Model.Layout{2n, 1n+3n})) ==
    Model.Layout{2n, 1n+3n} : Model.Layout}:
  Laws.reuse_free_slot(2n, 3n)
