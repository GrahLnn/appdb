use appdb::{Id, SqliteKey, SqliteModel, SqliteOrderKey, SqlitePageOrder, SqliteStore};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Entry {
    id: Id,
    created_at: i64,
    body: String,
}

fn store() -> SqliteStore<Entry> {
    let model = SqliteModel::json("entry", |entry: &Entry| SqliteKey::from(&entry.id))
        .expect("SQLite model table should be valid")
        .with_order(|entry: &Entry| Ok(SqliteOrderKey::Integer(entry.created_at)));
    SqliteStore::open_in_memory(model).expect("in-memory SQLite should open")
}

#[test]
fn sqlite_store_reuses_appdb_id_and_json_boundaries() {
    let mut store = store();
    let entries = (1..=5)
        .map(|id| Entry {
            id: Id::Number(id),
            created_at: id,
            body: format!("entry-{id}"),
        })
        .collect::<Vec<_>>();
    store
        .save_many(&entries)
        .expect("batch save should be atomic");

    assert!(
        store
            .exists()
            .expect("exists should query the compact table")
    );
    assert_eq!(store.list().expect("list should decode JSON"), entries);
    assert_eq!(
        store
            .get(&SqliteKey::Integer(3))
            .expect("get should use the typed key"),
        Some(entries[2].clone())
    );

    let first = store
        .page(2, None, SqlitePageOrder::Asc)
        .expect("first ascending page should be bounded");
    assert_eq!(first.items, entries[..2]);
    assert!(
        store
            .page(2, first.next.as_ref(), SqlitePageOrder::Desc)
            .is_err()
    );
    let second = store
        .page(2, first.next.as_ref(), SqlitePageOrder::Asc)
        .expect("cursor should continue from the last returned row");
    assert_eq!(second.items, entries[2..4]);
    let third = store
        .page(2, second.next.as_ref(), SqlitePageOrder::Asc)
        .expect("last page should be readable");
    assert_eq!(third.items, entries[4..]);
    assert!(third.next.is_none());

    let descending = store
        .page(2, None, SqlitePageOrder::Desc)
        .expect("descending page should use the same keyset index");
    assert_eq!(
        descending.items,
        entries.iter().rev().take(2).cloned().collect::<Vec<_>>()
    );

    let mut changed = entries[2].clone();
    changed.body = "updated".to_owned();
    store
        .update(&changed)
        .expect("update should replace one row");
    assert_eq!(store.get(&SqliteKey::Integer(3)).unwrap(), Some(changed));
    assert!(store.delete(&SqliteKey::Integer(1)).unwrap());
    assert!(!store.delete(&SqliteKey::Integer(1)).unwrap());
}
