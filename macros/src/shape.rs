use syn::{GenericArgument, PathArguments, Type, TypePath};

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum ContainerKind {
    Option,
    Vec,
}

/// Folds `Option<_>` and `Vec<_>` wrappers before applying a leaf operator.
///
/// The constructor algebra is deliberately explicit: most consumers rewrap
/// each constructor, while foreign leaf validation peels constructors to
/// recover the direct child type. Both interpretations share one traversal
/// without pretending that they have the same result shape.
pub(crate) fn fold_option_vec<F, C>(ty: &Type, leaf: &F, constructor: &C) -> Option<Type>
where
    F: Fn(&Type) -> Option<Type>,
    C: Fn(ContainerKind, Type) -> Type,
{
    if let Some((kind, inner)) = container_inner_type(ty) {
        let inner = fold_option_vec(inner, leaf, constructor)?;
        return Some(constructor(kind, inner));
    }

    leaf(ty)
}

/// Rebuilds each wrapper with the canonical generated-type paths.
pub(crate) fn map_option_vec<F>(ty: &Type, leaf: &F) -> Option<Type>
where
    F: Fn(&Type) -> Option<Type>,
{
    fold_option_vec(ty, leaf, &rewrap_container)
}

/// Removes only `Option<_>` / `Vec<_>` constructors while interpreting a leaf.
pub(crate) fn peel_option_vec<F>(ty: &Type, leaf: &F) -> Option<Type>
where
    F: Fn(&Type) -> Option<Type>,
{
    fold_option_vec(ty, leaf, &keep_inner)
}

fn rewrap_container(kind: ContainerKind, inner: Type) -> Type {
    match kind {
        ContainerKind::Option => syn::parse_quote!(::std::option::Option<#inner>),
        ContainerKind::Vec => syn::parse_quote!(::std::vec::Vec<#inner>),
    }
}

fn keep_inner(_kind: ContainerKind, inner: Type) -> Type {
    inner
}

pub(crate) fn option_inner_type(ty: &Type) -> Option<&Type> {
    container_inner_type(ty)
        .and_then(|(kind, inner)| (kind == ContainerKind::Option).then_some(inner))
}

pub(crate) fn vec_inner_type(ty: &Type) -> Option<&Type> {
    container_inner_type(ty).and_then(|(kind, inner)| (kind == ContainerKind::Vec).then_some(inner))
}

fn container_inner_type<'a>(ty: &'a Type) -> Option<(ContainerKind, &'a Type)> {
    let Type::Path(TypePath { path, .. }) = ty else {
        return None;
    };
    let segment = path.segments.last()?;
    let kind = if segment.ident == "Option" {
        ContainerKind::Option
    } else if segment.ident == "Vec" {
        ContainerKind::Vec
    } else {
        return None;
    };
    let PathArguments::AngleBracketed(args) = &segment.arguments else {
        return None;
    };
    let GenericArgument::Type(inner) = args.args.first()? else {
        return None;
    };
    Some((kind, inner))
}
