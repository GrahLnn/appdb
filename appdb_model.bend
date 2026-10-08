import Base

type Lifecycle is Data:
  Open{}
  Closed{}

def close(x: Lifecycle) -> Lifecycle:
  match x:
    case Open{}:
      Closed{}
    case Closed{}:
      Closed{}

type WritePlan is Data:
  Planned{writes: Nat}
  Committed{writes: Nat}
  Aborted{}

def commit(x: WritePlan) -> WritePlan:
  match x:
    case Planned{writes}:
      Committed{writes}
    case Committed{writes}:
      Committed{writes}
    case Aborted{}:
      Aborted{}

type Layout is Data:
  Layout{live: Nat, free: Nat}

def insert(x: Layout) -> Layout:
  match x:
    case Layout{live, free}:
      match free:
        case 0n:
          Layout{1n+live, 0n}
        case 1n+tail:
          Layout{1n+live, tail}

def remove(x: Layout) -> Layout:
  match x:
    case Layout{live, free}:
      match live:
        case 0n:
          Layout{0n, free}
        case 1n+tail:
          Layout{tail, 1n+free}
