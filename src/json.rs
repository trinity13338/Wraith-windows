//! Small helpers over serde_json values (key order is preserved, like the provider wrote it).

use serde_json::{Map, Value};

pub type Obj = Map<String, Value>;

pub trait Get {
    fn val(&self, key: &str) -> Option<&Value>;
    /// String form of a scalar (numbers and booleans included); "" for missing, null, objects and arrays.
    fn s(&self, key: &str) -> String {
        match self.val(key) {
            Some(Value::String(s)) => s.clone(),
            Some(Value::Number(n)) => n.to_string(),
            Some(Value::Bool(b)) => b.to_string(),
            _ => String::new(),
        }
    }
    fn s_or(&self, key: &str, fallback: &str) -> String {
        let v = self.s(key);
        if v.is_empty() && !matches!(self.val(key), Some(Value::String(_))) { fallback.to_string() } else { v }
    }
    /// First non-blank string among the keys.
    fn first(&self, keys: &[&str]) -> String {
        for k in keys {
            let v = self.s(k);
            if !v.trim().is_empty() { return v.trim().to_string(); }
        }
        String::new()
    }
    fn b(&self, key: &str, fallback: bool) -> bool {
        match self.val(key) {
            Some(Value::Bool(b)) => *b,
            Some(Value::Number(n)) => n.as_i64().map(|v| v != 0).unwrap_or(fallback),
            Some(Value::String(s)) => match s.trim().to_ascii_lowercase().as_str() { "true" | "1" => true, "false" | "0" => false, _ => fallback },
            _ => fallback,
        }
    }
    fn i(&self, key: &str, fallback: i64) -> i64 {
        match self.val(key) {
            Some(Value::Number(n)) => n.as_i64().or_else(|| n.as_f64().map(|f| f as i64)).unwrap_or(fallback),
            Some(Value::String(s)) => s.trim().parse().unwrap_or(fallback),
            _ => fallback,
        }
    }
    fn o(&self, key: &str) -> Option<&Obj> { self.val(key).and_then(|v| v.as_object()) }
    fn a(&self, key: &str) -> Option<&Vec<Value>> { self.val(key).and_then(|v| v.as_array()) }
    fn objects(&self, key: &str) -> Vec<&Obj> {
        self.a(key).map(|a| a.iter().filter_map(|v| v.as_object()).collect()).unwrap_or_default()
    }
    fn has(&self, key: &str) -> bool { self.val(key).is_some() }
}

impl Get for Obj {
    fn val(&self, key: &str) -> Option<&Value> { self.get(key) }
}
impl Get for Value {
    fn val(&self, key: &str) -> Option<&Value> { self.as_object().and_then(|o| o.get(key)) }
}
impl<T: Get> Get for Option<&T> {
    fn val(&self, key: &str) -> Option<&Value> { self.and_then(|o| o.val(key)) }
}

/// Mutable object child, created (or replaced if not an object).
pub fn child<'a>(o: &'a mut Obj, key: &str) -> &'a mut Obj {
    let entry = o.entry(key.to_string()).or_insert_with(|| Value::Object(Map::new()));
    if !entry.is_object() { *entry = Value::Object(Map::new()); }
    entry.as_object_mut().unwrap()
}
/// Mutable array child, created (or replaced if not an array).
pub fn child_list<'a>(o: &'a mut Obj, key: &str) -> &'a mut Vec<Value> {
    let entry = o.entry(key.to_string()).or_insert_with(|| Value::Array(vec![]));
    if !entry.is_array() { *entry = Value::Array(vec![]); }
    entry.as_array_mut().unwrap()
}
pub fn objects_mut(list: &mut [Value]) -> impl Iterator<Item = &mut Obj> { list.iter_mut().filter_map(|v| v.as_object_mut()) }

/// `obj!{"a" => 1, "b" => "x"}` → Obj
#[macro_export]
macro_rules! obj {
    () => { $crate::json::Obj::new() };
    ($($k:expr => $v:expr),+ $(,)?) => {{
        let mut m = $crate::json::Obj::new();
        $( m.insert(($k).to_string(), serde_json::json!($v)); )+
        m
    }};
}

pub fn parse_obj(text: &str) -> Result<Obj, String> {
    match serde_json::from_str::<Value>(text) {
        Ok(Value::Object(o)) => Ok(o),
        Ok(_) => Err(crate::tr!("Ожидается JSON-объект.", "A JSON object is expected.").into()),
        Err(e) => Err(crate::trf!("Некорректный JSON: {e}", "Invalid JSON: {e}")),
    }
}

/// Base64 as providers really send it: unpadded, URL-safe alphabet, wrapped lines.
pub fn base64_text(s: &str) -> Result<String, String> {
    use base64::Engine;
    let mut v: String = s.chars().filter(|c| !c.is_whitespace()).map(|c| match c { '-' => '+', '_' => '/', c => c }).collect();
    if let Some(cut) = v.find('=') { v.truncate(cut); }
    if v.is_empty() { return Err(crate::tr!("Пустые данные Base64.", "Empty Base64 data.").into()); }
    let bytes = base64::engine::general_purpose::STANDARD_NO_PAD.decode(v.as_bytes()).map_err(|_| crate::tr!("Некорректный Base64.", "Invalid Base64.").to_string())?;
    String::from_utf8(bytes).map_err(|_| crate::tr!("Base64 не содержит текст.", "Base64 does not contain text.").to_string())
}

/// Text a person could have written, not binary noise decoded from base64.
pub fn readable(s: &str) -> bool {
    !s.trim().is_empty() && s.chars().all(|c| c != '\u{FFFD}' && (c >= ' ' || c == '\n' || c == '\r' || c == '\t'))
}

pub fn pretty(v: &impl serde::Serialize) -> String { serde_json::to_string_pretty(v).unwrap_or_default() }
