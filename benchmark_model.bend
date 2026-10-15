import Base

# Pointwise projection of a key/value map onto any two distinct keys.
# All other keys are unchanged by these operations.
type Cell is Data:
  Missing{}
  Present{value: Nat}

type Entries is Data:
  Entries{left: Cell, right: Cell}

def put_left(entries: Entries, value: Nat) -> Entries:
  match entries:
    case Entries{left, right}:
      Entries{Present{value}, right}

def put_right(entries: Entries, value: Nat) -> Entries:
  match entries:
    case Entries{left, right}:
      Entries{left, Present{value}}

def delete_left(entries: Entries) -> Entries:
  match entries:
    case Entries{left, right}:
      Entries{Missing{}, right}
