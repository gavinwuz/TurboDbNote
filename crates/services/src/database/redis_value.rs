use super::workbench::Rows;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ValuePage {
    pub rows: Rows,
    pub kind: String,
    pub ttl: i64,
    pub cursor: Option<String>,
    pub offset: usize,
    pub text: Option<String>,
    pub truncated: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Mutation {
    pub key: Vec<u8>,
    pub kind: String,
    pub member: String,
    pub expected: Option<String>,
    pub value: String,
    pub delete: bool,
}
impl Mutation {
    pub fn validate(&self) -> Result<(), String> {
        if self.key.is_empty()
            || self.key.len() > 4096
            || self.member.len() > 65536
            || self.value.len() > 65536
            || self.expected.as_ref().is_some_and(|v| v.len() > 65536)
            || !["none", "string", "hash", "set", "zset", "list"].contains(&self.kind.as_str())
            || (self.kind == "zset"
                && !self.delete
                && !self.value.parse::<f64>().is_ok_and(f64::is_finite))
            || (self.kind == "list"
                && self.expected.is_some()
                && self.member.parse::<u64>().is_err())
        {
            return Err("redis.write_invalid".into());
        }
        Ok(())
    }
}

/// Atomic compare/write. String replacement preserves its remaining expiry, including Redis 5.
pub const WRITE_VALUE: &str = r#"
local key=KEYS[1]
local kind=redis.call('TYPE',key).ok
if kind~=ARGV[1] then return 0 end
if ARGV[2]=='delete' then redis.call('DEL',key) return 1 end
local member=ARGV[3]
local expected=ARGV[4]
local has=ARGV[5]=='1'
local value=ARGV[6]
local old=false
if kind=='string' then
  if redis.call('STRLEN',key)>65536 then return 0 end
  old=redis.call('GET',key)
elseif kind=='hash' then old=redis.call('HGET',key,member)
elseif kind=='zset' then old=redis.call('ZSCORE',key,member)
elseif kind=='set' then old=redis.call('SISMEMBER',key,member)==1 and member or false
elseif kind=='list' and has then old=redis.call('LINDEX',key,member)
end
if has and old~=expected then return 0 end
if not has and old then return 0 end
local original_ttl=redis.call('PTTL',key)
if kind=='none' then redis.call('SET',key,value,'NX')
elseif kind=='string' then
  local ttl=redis.call('PTTL',key)
  redis.call('SET',key,value)
  if ttl>=0 then redis.call('PEXPIRE',key,ttl) end
elseif kind=='hash' then redis.call('HSET',key,member,value)
elseif kind=='zset' then redis.call('ZADD',key,value,member)
elseif kind=='set' then
  redis.call('SADD',key,value)
  if has and member~=value then redis.call('SREM',key,member) end
elseif kind=='list' then
  if has then redis.call('LSET',key,member,value) else redis.call('RPUSH',key,value) end
end
if original_ttl>=0 then redis.call('PEXPIRE',key,original_ttl) end
return 1
"#;
pub fn display(bytes: &[u8]) -> String {
    match std::str::from_utf8(bytes) {
        Ok(s)
            if !s
                .chars()
                .any(|c| c.is_control() && !matches!(c, '\r' | '\n' | '\t')) =>
        {
            s.into()
        }
        _ => format!(
            "hex:{}",
            bytes.iter().map(|b| format!("{b:02x}")).collect::<String>()
        ),
    }
}

// Read-only operations only. Bound the returned bytes inside Redis, before transfer to
// the client. COUNT is advisory; compact encodings can return more than requested.
pub const READ_PAGE: &str = r#"
local key=KEYS[1]
local kind=redis.call('TYPE',key).ok
local ttl=redis.call('TTL',key)
local size=tonumber(ARGV[2])
local offset=tonumber(ARGV[3])
local limit=65536
local rows={}
local used=0
local truncated=0
local next='0'
local function add(a,b)
  if used>=limit or #rows>=1000 then truncated=1 return end
  local left=limit-used
  if #a>left then a=string.sub(a,1,left) truncated=1 end
  used=used+#a
  if b then
    left=limit-used
    if #b>left then b=string.sub(b,1,left) truncated=1 end
    used=used+#b
    table.insert(rows,{a,b})
  else table.insert(rows,{a}) end
end
if kind=='string' then
  local length=redis.call('STRLEN',key)
  add(redis.call('GETRANGE',key,0,limit-1))
  if length>limit then truncated=1 end
elseif kind=='hash' or kind=='set' or kind=='zset' then
  local cmd=kind=='hash' and 'HSCAN' or (kind=='set' and 'SSCAN' or 'ZSCAN')
  local scan=redis.call(cmd,key,ARGV[1],'COUNT',size)
  next=scan[1]
  local step=kind=='set' and 1 or 2
  for i=1,#scan[2],step do add(scan[2][i],step==2 and scan[2][i+1] or nil) end
elseif kind=='list' then
  local values=redis.call('LRANGE',key,offset,offset+size-1)
  for i,value in ipairs(values) do add(tostring(offset+i-1),value) end
  if offset+#values<redis.call('LLEN',key) then next=tostring(offset+#values) end
elseif kind~='none' then
  return redis.error_reply('Unsupported key type')
end
return {kind,ttl,next,truncated,rows}
"#;

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn binary_and_text_are_distinguishable() {
        assert_eq!(display(b"hello"), "hello");
        assert_eq!(display(&[0, 255]), "hex:00ff");
        assert!(READ_PAGE.contains("GETRANGE"));
        assert!(!READ_PAGE.contains("redis.call('SET'"));
    }
}
