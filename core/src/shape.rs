use std::future::Future;

/// Applies one fallible asynchronous operation to an optional value.
///
/// `T` is deliberately the value passed to the operator, so callers can use
/// this for both owned values and borrowed values such as `Option<&T>` without
/// introducing a second traversal implementation.
pub(crate) async fn try_map_option<T, U, F, Fut>(
    value: Option<T>,
    operation: F,
) -> anyhow::Result<Option<U>>
where
    F: FnOnce(T) -> Fut,
    Fut: Future<Output = anyhow::Result<U>>,
{
    match value {
        Some(value) => Ok(Some(operation(value).await?)),
        None => Ok(None),
    }
}

/// Applies one fallible asynchronous operation to an exact-size sequence.
///
/// The iterator is consumed exactly once. Its known length supplies the same
/// allocation bound as the former `Vec`-specific loops, while the sequential
/// await preserves side-effect order and first-error short circuiting for
/// persistence, hydration, and lookup operators.
pub(crate) async fn try_map_sequence<I, U, F, Fut>(
    values: I,
    mut operation: F,
) -> anyhow::Result<Vec<U>>
where
    I: IntoIterator,
    I::IntoIter: ExactSizeIterator,
    F: FnMut(I::Item) -> Fut,
    Fut: Future<Output = anyhow::Result<U>>,
{
    let iterator = values.into_iter();
    let mut output = Vec::with_capacity(iterator.len());
    for value in iterator {
        output.push(operation(value).await?);
    }
    Ok(output)
}

#[cfg(test)]
#[path = "shape_tests.rs"]
mod tests;
