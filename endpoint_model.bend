import Base

# The endpoint adapter must keep a native path as one value. The external
# text parser correspondence is checked by native persistence tests.
type Segments is Data:
  Empty{}
  Segment{value: Nat, tail: Segments}

type NativePath is Data:
  NativePath{drive: Nat, parts: Segments}

type Endpoint is Data:
  Opaque{backend: Nat, path: NativePath}

def encode(backend: Nat, path: NativePath) -> Endpoint:
  Opaque{backend, path}

def path(endpoint: Endpoint) -> NativePath:
  match endpoint:
    case Opaque{backend, path}:
      path
