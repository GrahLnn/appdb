use std::collections::{BTreeMap, BTreeSet};

use anyhow::Result;
use surrealdb::types::{RecordId, Table, ToSql};

use crate::error::DBError;
use crate::query::{RawSqlStmt, query_bound_checked};
use crate::{RelationWrite, RelationWriteDirection};

#[derive(Debug)]
struct RelationWriteBatch<'a> {
    relation: &'a str,
    direction: RelationWriteDirection,
    records: Vec<RecordId>,
    edges: Vec<crate::graph::OrderedRelationEdge>,
}

fn relation_write_batches(writes: &[RelationWrite]) -> Result<Vec<RelationWriteBatch<'_>>> {
    let mut groups = Vec::new();
    let mut index_by_key = BTreeMap::new();

    for write in writes {
        let key = (
            write.relation,
            match write.direction {
                RelationWriteDirection::Outgoing => 0u8,
                RelationWriteDirection::Incoming => 1u8,
            },
        );

        let group_index = if let Some(index) = index_by_key.get(&key) {
            *index
        } else {
            let index = groups.len();
            groups.push(RelationWriteBatch {
                relation: write.relation,
                direction: write.direction,
                records: Vec::new(),
                edges: Vec::new(),
            });
            index_by_key.insert(key, index);
            index
        };

        let batch = groups
            .get_mut(group_index)
            .expect("relation batch index should remain valid");
        batch.records.push(write.record.clone());
        batch.edges.extend(write.edges.iter().cloned());
    }

    for batch in &groups {
        for edge in &batch.edges {
            if edge._in.is_none() {
                return Err(DBError::InvalidModel(
                    "relation edge write is missing its source record id".to_owned(),
                )
                .into());
            }
        }
    }

    Ok(groups)
}

pub(crate) async fn ensure_relation_tables(writes: &[RelationWrite]) -> Result<()> {
    let relations = writes
        .iter()
        .map(|write| write.relation)
        .collect::<BTreeSet<_>>();
    if relations.is_empty() {
        return Ok(());
    }

    let mut stmt = RawSqlStmt::new("BEGIN TRANSACTION;");
    for relation in relations {
        let relation_sql = Table::from(relation).to_sql();
        stmt.sql.push_str(&format!(
            "DEFINE TABLE IF NOT EXISTS {relation_sql} TYPE RELATION SCHEMALESS;"
        ));
    }
    stmt.sql.push_str("COMMIT TRANSACTION;");

    query_bound_checked(stmt).await?;
    Ok(())
}

pub(crate) fn append_relation_sync_to_stmt(
    mut stmt: RawSqlStmt,
    writes: &[RelationWrite],
    prefix: &str,
) -> Result<(RawSqlStmt, usize)> {
    let batches = relation_write_batches(writes)?;
    let mut statement_count = 0usize;

    for (batch_idx, batch) in batches.iter().enumerate() {
        let field = match batch.direction {
            RelationWriteDirection::Outgoing => "in",
            RelationWriteDirection::Incoming => "out",
        };
        let relation_sql = Table::from(batch.relation).to_sql();
        stmt.sql
            .push_str(&format!("DELETE FROM {relation_sql} WHERE "));
        for (record_idx, _) in batch.records.iter().enumerate() {
            if record_idx > 0 {
                stmt.sql.push_str(" OR ");
            }
            stmt.sql.push_str(&format!(
                "{field} = ${prefix}_record_{batch_idx}_{record_idx}"
            ));
        }
        stmt.sql.push_str(" RETURN NONE;");
        statement_count += 1;
    }

    for (batch_idx, batch) in batches.iter().enumerate() {
        if batch.edges.is_empty() {
            continue;
        }

        let relation_sql = Table::from(batch.relation).to_sql();
        for (edge_idx, _) in batch.edges.iter().enumerate() {
            stmt.sql.push_str(&format!(
                "RELATE ${prefix}_in_{batch_idx}_{edge_idx} -> {relation_sql} -> ${prefix}_out_{batch_idx}_{edge_idx} SET position = ${prefix}_position_{batch_idx}_{edge_idx};"
            ));
            statement_count += 1;
        }
    }

    for (batch_idx, batch) in batches.iter().enumerate() {
        for (record_idx, record) in batch.records.iter().enumerate() {
            stmt = stmt.bind(
                format!("{prefix}_record_{batch_idx}_{record_idx}"),
                record.clone(),
            );
        }
        for (edge_idx, edge) in batch.edges.iter().enumerate() {
            let in_record = edge._in.clone().ok_or_else(|| {
                DBError::InvalidModel(
                    "relation edge write is missing its source record id".to_owned(),
                )
            })?;
            stmt = stmt
                .bind(format!("{prefix}_in_{batch_idx}_{edge_idx}"), in_record)
                .bind(
                    format!("{prefix}_out_{batch_idx}_{edge_idx}"),
                    edge.out.clone(),
                )
                .bind(
                    format!("{prefix}_position_{batch_idx}_{edge_idx}"),
                    edge.position,
                );
        }
    }

    Ok((stmt, statement_count))
}

pub(crate) fn append_relation_sync_with_anchor_expr_to_stmt(
    mut stmt: RawSqlStmt,
    writes: &[RelationWrite],
    prefix: &str,
    anchor_expr: &str,
) -> Result<(RawSqlStmt, usize)> {
    let batches = relation_write_batches(writes)?;
    let mut statement_count = 0usize;

    for batch in &batches {
        let field = match batch.direction {
            RelationWriteDirection::Outgoing => "in",
            RelationWriteDirection::Incoming => "out",
        };
        let relation_sql = Table::from(batch.relation).to_sql();
        stmt.sql
            .push_str(&format!("DELETE FROM {relation_sql} WHERE "));
        stmt.sql.push_str(&format!("{field} = {anchor_expr}"));
        stmt.sql.push_str(" RETURN NONE;");
        statement_count += 1;
    }

    for (batch_idx, batch) in batches.iter().enumerate() {
        if batch.edges.is_empty() {
            continue;
        }

        let relation_sql = Table::from(batch.relation).to_sql();
        for (edge_idx, _) in batch.edges.iter().enumerate() {
            match batch.direction {
                RelationWriteDirection::Outgoing => stmt.sql.push_str(&format!(
                    "RELATE {anchor_expr} -> {relation_sql} -> ${prefix}_target_{batch_idx}_{edge_idx} SET position = ${prefix}_position_{batch_idx}_{edge_idx};"
                )),
                RelationWriteDirection::Incoming => stmt.sql.push_str(&format!(
                    "RELATE ${prefix}_source_{batch_idx}_{edge_idx} -> {relation_sql} -> {anchor_expr} SET position = ${prefix}_position_{batch_idx}_{edge_idx};"
                )),
            }
            statement_count += 1;
        }
    }

    for (_batch_idx, batch) in batches.iter().enumerate() {
        for (edge_idx, edge) in batch.edges.iter().enumerate() {
            match batch.direction {
                RelationWriteDirection::Outgoing => {
                    stmt = stmt.bind(
                        format!("{prefix}_target_{_batch_idx}_{edge_idx}"),
                        edge.out.clone(),
                    );
                }
                RelationWriteDirection::Incoming => {
                    let in_record = edge._in.clone().ok_or_else(|| {
                        DBError::InvalidModel(
                            "relation edge write is missing its source record id".to_owned(),
                        )
                    })?;
                    stmt = stmt.bind(
                        format!("{prefix}_source_{_batch_idx}_{edge_idx}"),
                        in_record,
                    );
                }
            }
            stmt = stmt.bind(
                format!("{prefix}_position_{_batch_idx}_{edge_idx}"),
                edge.position,
            );
        }
    }

    Ok((stmt, statement_count))
}
