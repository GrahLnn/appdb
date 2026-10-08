import Base
import ./LAWS.bend as Laws
import ./appdb_model.bend as Model

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

def main() -> {Model.remove(Model.insert(Model.Layout{2n, 1n+3n})) ==
    Model.Layout{2n, 1n+3n} : Model.Layout}:
  Laws.reuse_free_slot(2n, 3n)
