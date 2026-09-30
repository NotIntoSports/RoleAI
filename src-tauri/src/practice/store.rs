//! 题单落库：practice_plans 表的保存/查询/删除。
//! 复用 sessions/store.rs 的门面风格：借用 Database，闭包内是 rusqlite::Result。

use rusqlite::{OptionalExtension, params};

use crate::database::{Database, DatabaseError};

use super::dto::{PracticePlan, PracticePlanSummary, PracticeQuestion};

pub struct PracticePlanStore<'a> {
    database: &'a Database,
}

impl<'a> PracticePlanStore<'a> {
    pub fn new(database: &'a Database) -> Self {
        Self { database }
    }

    /// 保存（upsert）：存在则整体覆盖并刷新 updated_at。
    pub fn save(&self, plan: &PracticePlan) -> Result<(), DatabaseError> {
        let questions_json =
            serde_json::to_string(&plan.questions).map_err(|_| DatabaseError::Operation)?;
        self.database.with_connection(|connection| {
            connection.execute(
                "INSERT INTO practice_plans(
                    id, title, position, interviewer_style, difficulty,
                    question_count, questions_json, created_at, updated_at
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
                 ON CONFLICT(id) DO UPDATE SET
                    title = excluded.title,
                    position = excluded.position,
                    interviewer_style = excluded.interviewer_style,
                    difficulty = excluded.difficulty,
                    question_count = excluded.question_count,
                    questions_json = excluded.questions_json,
                    updated_at = excluded.updated_at",
                params![
                    plan.id,
                    plan.title,
                    plan.position,
                    plan.interviewer_style,
                    plan.difficulty,
                    plan.question_count() as i64,
                    questions_json,
                    plan.created_at,
                    plan.updated_at,
                ],
            )?;
            Ok(())
        })
    }

    pub fn get(&self, id: &str) -> Result<Option<PracticePlan>, DatabaseError> {
        self.database.with_connection(|connection| {
            connection
                .query_row(
                    "SELECT id, title, position, interviewer_style, difficulty,
                            questions_json, created_at, updated_at
                     FROM practice_plans WHERE id = ?1",
                    params![id],
                    map_plan,
                )
                .optional()
        })
    }

    pub fn list(&self) -> Result<Vec<PracticePlanSummary>, DatabaseError> {
        self.database.with_connection(|connection| {
            let mut statement = connection.prepare(
                "SELECT id, title, position, interviewer_style, difficulty,
                        question_count, created_at, updated_at
                 FROM practice_plans ORDER BY updated_at DESC, id",
            )?;
            let rows = statement.query_map([], |row| {
                Ok(PracticePlanSummary {
                    id: row.get(0)?,
                    title: row.get(1)?,
                    position: row.get(2)?,
                    interviewer_style: row.get(3)?,
                    difficulty: row.get(4)?,
                    question_count: row.get::<_, i64>(5).unwrap_or(0).max(0) as usize,
                    created_at: row.get(6)?,
                    updated_at: row.get(7)?,
                })
            })?;
            rows.collect()
        })
    }

    pub fn delete(&self, id: &str) -> Result<bool, DatabaseError> {
        self.database.with_connection(|connection| {
            let deleted =
                connection.execute("DELETE FROM practice_plans WHERE id = ?1", params![id])?;
            Ok(deleted > 0)
        })
    }
}

fn map_plan(row: &rusqlite::Row<'_>) -> rusqlite::Result<PracticePlan> {
    let questions_json: String = row.get(5)?;
    let questions: Vec<PracticeQuestion> =
        serde_json::from_str(&questions_json).unwrap_or_default();
    Ok(PracticePlan {
        id: row.get(0)?,
        title: row.get(1)?,
        position: row.get(2)?,
        interviewer_style: row.get(3)?,
        difficulty: row.get(4)?,
        questions,
        created_at: row.get(6)?,
        updated_at: row.get(7)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn database() -> (tempfile::TempDir, Database) {
        let directory = tempfile::tempdir().unwrap();
        let database = Database::open(directory.path().join("app.sqlite3")).unwrap();
        database.migrate().unwrap();
        (directory, database)
    }

    fn sample_plan(id: &str) -> PracticePlan {
        PracticePlan {
            id: id.to_owned(),
            title: "后端一面".into(),
            position: "后端工程师".into(),
            interviewer_style: "严苛面试官".into(),
            difficulty: "standard".into(),
            questions: vec![PracticeQuestion {
                prompt: "介绍一个你负责的项目".into(),
                focus: "项目深度".into(),
                expected_points: vec!["背景".into(), "结果".into()],
                followups: vec!["最大的困难是什么".into()],
            }],
            created_at: "2026-10-01T00:00:00Z".into(),
            updated_at: "2026-10-01T00:00:00Z".into(),
        }
    }

    #[test]
    fn save_get_list_delete_roundtrip() {
        let (_directory, database) = database();
        let store = PracticePlanStore::new(&database);

        store.save(&sample_plan("plan-1")).unwrap();
        let plan = store.get("plan-1").unwrap().expect("plan exists");
        assert_eq!(plan.title, "后端一面");
        assert_eq!(plan.questions.len(), 1);
        assert_eq!(plan.questions[0].prompt, "介绍一个你负责的项目");

        let list = store.list().unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].id, "plan-1");
        assert_eq!(list[0].question_count, 1);
        assert_eq!(list[0].position, "后端工程师");

        assert!(store.delete("plan-1").unwrap());
        assert!(!store.delete("plan-1").unwrap());
        assert!(store.get("plan-1").unwrap().is_none());
    }

    #[test]
    fn save_upserts_over_fields_and_refreshes_updated_at() {
        let (_directory, database) = database();
        let store = PracticePlanStore::new(&database);
        store.save(&sample_plan("plan-1")).unwrap();

        let mut edited = sample_plan("plan-1");
        edited.title = "后端二面".into();
        edited.updated_at = "2026-10-02T00:00:00Z".into();
        edited.questions.push(PracticeQuestion {
            prompt: "如何设计限流".into(),
            focus: String::new(),
            expected_points: Vec::new(),
            followups: Vec::new(),
        });
        store.save(&edited).unwrap();

        let plan = store.get("plan-1").unwrap().expect("plan exists");
        assert_eq!(plan.title, "后端二面");
        assert_eq!(plan.updated_at, "2026-10-02T00:00:00Z");
        assert_eq!(plan.questions.len(), 2);
        assert_eq!(store.list().unwrap().len(), 1);
    }

    #[test]
    fn missing_plan_is_none_and_corrupt_json_degrades_to_empty() {
        let (_directory, database) = database();
        let store = PracticePlanStore::new(&database);
        assert!(store.get("missing").unwrap().is_none());

        store.save(&sample_plan("plan-1")).unwrap();
        database
            .with_connection(|connection| {
                connection.execute_batch(
                    "UPDATE practice_plans SET questions_json = '{oops}' WHERE id = 'plan-1'",
                )
            })
            .unwrap();
        let plan = store.get("plan-1").unwrap().expect("row still exists");
        assert!(plan.questions.is_empty());
    }
}
