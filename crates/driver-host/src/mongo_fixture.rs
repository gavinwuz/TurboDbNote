//! Isolated loopback wire fixture for protocol tests and manual UI verification.
//! Never connects to an external database; documents exist only in this process.
use mongodb::bson::{self, Bson, Document, doc};
use std::{
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread,
};
pub struct Fixture {
    pub port: u16,
    stop: Arc<AtomicBool>,
    pub state: Arc<Mutex<State>>,
}
pub struct State {
    pub documents: Vec<Document>,
    pub writes: usize,
    pub finds: usize,
    pub deny: bool,
    pub view: bool,
}
impl Fixture {
    pub fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let port = listener.local_addr().unwrap().port();
        let stop = Arc::new(AtomicBool::new(false));
        let state=Arc::new(Mutex::new(State{documents:(0..8).map(|i|doc!{"_id":bson::oid::ObjectId::new(),"r":i,"l":56-i,"cell":(0..56-i).map(|n|Bson::Document(doc!{"r":i,"c":n,"v":format!("cell-{n}")})).collect::<Vec<_>>(),"long":Bson::Int64(i64::MAX),"nullable":Bson::Null,"date":bson::DateTime::from_millis(1721485800000)}).collect(),writes:0,finds:0,deny:false,view:false}));
        let halted = stop.clone();
        let shared = state.clone();
        thread::spawn(move || {
            while !halted.load(Ordering::Relaxed) {
                match listener.accept() {
                    Ok((stream, _)) => {
                        let state = shared.clone();
                        let stop = halted.clone();
                        thread::spawn(move || serve(stream, state, stop));
                    }
                    Err(_) => thread::sleep(std::time::Duration::from_millis(10)),
                }
            }
        });
        Self { port, stop, state }
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}
fn read_doc(bytes: &[u8], pos: &mut usize) -> Option<Document> {
    let len = i32::from_le_bytes(bytes.get(*pos..*pos + 4)?.try_into().ok()?) as usize;
    if len < 5 {
        return None;
    }
    let d = bson::from_slice(bytes.get(*pos..*pos + len)?).ok()?;
    *pos += len;
    Some(d)
}
fn request(opcode: i32, body: &[u8]) -> Option<Document> {
    if opcode == 2004 {
        let mut pos = 4;
        while *body.get(pos)? != 0 {
            pos += 1;
        }
        pos += 9;
        return read_doc(body, &mut pos);
    }
    if opcode != 2013 {
        return None;
    }
    let mut pos = 4;
    let mut request = Document::new();
    while pos < body.len() {
        let kind = *body.get(pos)?;
        pos += 1;
        if kind == 0 {
            request.extend(read_doc(body, &mut pos)?);
        } else if kind == 1 {
            let size = i32::from_le_bytes(body.get(pos..pos + 4)?.try_into().ok()?) as usize;
            let end = pos + size;
            pos += 4;
            let start = pos;
            while *body.get(pos)? != 0 {
                pos += 1;
            }
            let key = std::str::from_utf8(body.get(start..pos)?).ok()?.to_owned();
            pos += 1;
            let mut docs = Vec::new();
            while pos < end {
                docs.push(Bson::Document(read_doc(body, &mut pos)?));
            }
            request.insert(key, docs);
        } else {
            return None;
        }
    }
    Some(request)
}
fn serve(mut stream: TcpStream, state: Arc<Mutex<State>>, stop: Arc<AtomicBool>) {
    let _ = stream.set_nonblocking(false);
    let _ = stream.set_read_timeout(Some(std::time::Duration::from_secs(2)));
    let _ = stream.set_write_timeout(Some(std::time::Duration::from_secs(2)));
    while !stop.load(Ordering::Relaxed) {
        let mut header = [0; 16];
        match stream.read_exact(&mut header[..1]) {
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) =>
            {
                continue;
            }
            Err(_) => break,
            Ok(()) => {}
        }
        if stream.read_exact(&mut header[1..]).is_err() {
            break;
        }

        let size = i32::from_le_bytes(header[0..4].try_into().unwrap());
        if !(16..=1048576).contains(&size) {
            break;
        }
        let mut body = vec![0; size as usize - 16];
        if stream.read_exact(&mut body).is_err() {
            break;
        }
        let opcode = i32::from_le_bytes(header[12..16].try_into().unwrap());
        let Some(request) = request(opcode, &body) else {
            break;
        };
        let response = respond(&request, &mut state.lock().unwrap());
        let Ok(bytes) = bson::to_vec(&response) else {
            break;
        };
        let mut response = Vec::new();
        let response_opcode = if opcode == 2004 { 1i32 } else { 2013 };
        let prefix = if opcode == 2004 { 20 } else { 5 };
        response.extend_from_slice(&((16 + prefix + bytes.len()) as i32).to_le_bytes());
        response.extend_from_slice(&1i32.to_le_bytes());
        response.extend_from_slice(&header[4..8]);
        response.extend_from_slice(&response_opcode.to_le_bytes());
        if opcode == 2004 {
            response.extend_from_slice(&[0; 16]);
            response.extend_from_slice(&1i32.to_le_bytes());
        } else {
            response.extend_from_slice(&[0; 5]);
        }
        response.extend_from_slice(&bytes);
        if stream.write_all(&response).is_err() {
            break;
        }
    }
}
fn respond(request: &Document, state: &mut State) -> Document {
    let command = request.keys().next().map(String::as_str).unwrap_or("");
    match command {
        "isMaster" | "ismaster" | "hello" => {
            doc! {"ok":1.,"ismaster":true,"isWritablePrimary":true,"minWireVersion":0,"maxWireVersion":25,"maxBsonObjectSize":16777216,"maxMessageSizeBytes":48000000,"maxWriteBatchSize":100000}
        }
        "ping" | "endSessions" | "killCursors" => doc! {"ok":1.},
        "listDatabases" => doc! {"ok":1.,"databases":[{"name":"fixture"}],"totalSize":0},
        "listCollections" => {
            doc! {"ok":1.,"cursor":{"id":0i64,"ns":"fixture.$cmd.listCollections","firstBatch":[{"name":"sample","type":if state.view{"view"}else{"collection"},"options":{},"info":{"readOnly":state.view,"uuid":bson::Binary{subtype:bson::spec::BinarySubtype::Uuid,bytes:vec![0;16]}}}]}}
        }
        "find" => {
            state.finds += 1;
            let skip = request
                .get_i64("skip")
                .ok()
                .or_else(|| request.get_i32("skip").ok().map(i64::from))
                .unwrap_or(0) as usize;
            let limit = request
                .get_i64("limit")
                .ok()
                .or_else(|| request.get_i32("limit").ok().map(i64::from))
                .unwrap_or(200) as usize;
            let docs = state
                .documents
                .iter()
                .skip(skip)
                .take(limit)
                .cloned()
                .map(Bson::Document)
                .collect::<Vec<_>>();
            doc! {"ok":1.,"cursor":{"id":0i64,"ns":"fixture.sample","firstBatch":docs}}
        }
        "aggregate" => {
            doc! {"ok":1.,"cursor":{"id":0i64,"ns":"fixture.sample","firstBatch":[{"n":state.documents.len() as i64}]}}
        }
        "update" if state.deny => doc! {"ok":0.,"code":13,"errmsg":"fixture permission denied"},
        "update" => {
            let Some(Bson::Document(change)) =
                request.get_array("updates").ok().and_then(|a| a.first())
            else {
                return doc! {"ok":0.,"code":9,"errmsg":"invalid fixture request"};
            };
            let expected = change
                .get_document("q")
                .ok()
                .and_then(|q| q.get_document("$expr").ok())
                .and_then(|e| e.get_array("$eq").ok())
                .and_then(|a| a.get(1))
                .and_then(Bson::as_document)
                .and_then(|d| d.get_document("$literal").ok());
            let replacement = change.get_document("u").ok();
            let index = expected.and_then(|old| state.documents.iter().position(|doc| doc == old));
            let n = if let (Some(index), Some(new)) = (index, replacement) {
                state.documents[index] = new.clone();
                state.writes += 1;
                1
            } else {
                0
            };
            doc! {"ok":1.,"n":n,"nModified":n}
        }
        "insert" if state.deny => doc! {"ok":0.,"code":13,"errmsg":"fixture permission denied"},
        "insert" => {
            let Some(documents) = request.get_array("documents").ok() else {
                return doc! {"ok":0.,"code":9};
            };
            for value in documents {
                if let Bson::Document(doc) = value {
                    state.documents.push(doc.clone());
                    state.writes += 1;
                }
            }
            doc! {"ok":1.,"n":documents.len() as i32}
        }
        _ => doc! {"ok":0.,"code":59,"errmsg":"unsupported fixture operation"},
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn reads_bson_command_sequence() {
        let doc = doc! {"insert":"sample"};
        let data = doc! {"n":1};
        let mut body = vec![0; 5];
        body.extend(bson::to_vec(&doc).unwrap());
        body.push(1);
        let data = bson::to_vec(&data).unwrap();
        body.extend(((4 + 10 + data.len()) as i32).to_le_bytes());
        body.extend(b"documents\0");
        body.extend(data);
        assert_eq!(
            request(2013, &body)
                .unwrap()
                .get_array("documents")
                .unwrap()
                .len(),
            1
        );
    }
}
