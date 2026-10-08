import Base
import ./appdb_model.bend as Model

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
