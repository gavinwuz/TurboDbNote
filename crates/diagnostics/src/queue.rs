use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use std::{path::Path, time::Duration};

#[derive(Clone, Serialize, Deserialize)]
pub struct EventRecord {
    pub id: String,
    pub session: String,
    pub name: String,
    pub version: String,
    pub timestamp_ms: u64,
    pub elapsed_ms: u64,
}
pub(crate) struct Queue(Connection);
impl Queue {
    pub fn open(path: &Path) -> Result<Self, String> {
        let connection = Connection::open(path).map_err(|_| "Unable to open diagnostic queue")?;
        connection
            .busy_timeout(Duration::from_millis(250))
            .map_err(|_| "Unable to configure diagnostic queue")?;
        connection.execute_batch("PRAGMA journal_mode=WAL; CREATE TABLE IF NOT EXISTS events (seq INTEGER PRIMARY KEY, payload TEXT NOT NULL);").map_err(|_| "Unable to initialize diagnostic queue")?;
        Ok(Self(connection))
    }
    pub fn push(&mut self, record: &EventRecord) -> Result<(), String> {
        let payload =
            serde_json::to_string(record).map_err(|_| "Unable to encode diagnostic event")?;
        let tx = self
            .0
            .transaction()
            .map_err(|_| "Diagnostic queue is busy")?;
        tx.execute("INSERT INTO events(payload) VALUES(?1)", [payload])
            .map_err(|_| "Unable to save diagnostic event")?;
        tx.execute("DELETE FROM events WHERE seq NOT IN (SELECT seq FROM events ORDER BY seq DESC LIMIT 1000)",[]).map_err(|_| "Unable to trim diagnostic queue")?;
        tx.commit()
            .map_err(|_| "Unable to commit diagnostic event".into())
    }
    pub fn batch(&self) -> Result<Vec<(i64, EventRecord)>, String> {
        let mut statement = self
            .0
            .prepare("SELECT seq,payload FROM events ORDER BY seq LIMIT 50")
            .map_err(|_| "Unable to read diagnostic queue")?;
        let rows = statement
            .query_map([], |row| {
                Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
            })
            .map_err(|_| "Unable to read diagnostic events")?;
        rows.map(|row| {
            let (seq, payload) = row.map_err(|_| "Invalid diagnostic row")?;
            let event = serde_json::from_str(&payload).map_err(|_| "Invalid diagnostic event")?;
            Ok((seq, event))
        })
        .collect()
    }
    pub fn acknowledge(&mut self, seqs: &[i64]) -> Result<(), String> {
        let tx = self
            .0
            .transaction()
            .map_err(|_| "Diagnostic queue is busy")?;
        for seq in seqs {
            tx.execute("DELETE FROM events WHERE seq=?1", [seq])
                .map_err(|_| "Unable to acknowledge diagnostic event")?;
        }
        tx.commit()
            .map_err(|_| "Unable to commit diagnostic acknowledgements".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn record(i: u64) -> EventRecord {
        EventRecord {
            id: i.to_string(),
            session: "session".into(),
            name: "app_started".into(),
            version: "1".into(),
            timestamp_ms: i,
            elapsed_ms: 0,
        }
    }
    #[test]
    fn bounds_queue_and_only_deletes_acknowledged_rows() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("events.db");
        let mut queue = Queue::open(&path).unwrap();
        for i in 0..1005 {
            queue.push(&record(i)).unwrap();
        }
        let batch = queue.batch().unwrap();
        assert_eq!(batch.len(), 50);
        assert_eq!(batch[0].1.id, "5");
        queue.acknowledge(&[batch[0].0, batch[2].0]).unwrap();
        drop(queue);
        let queue = Queue::open(&path).unwrap();
        assert_eq!(queue.batch().unwrap()[0].1.id, "6");
    }
}
