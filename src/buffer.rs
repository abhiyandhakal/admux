use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};

pub const MAX_BUFFERS: usize = 50;
pub const MAX_BUFFER_BYTES: usize = 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PasteBuffer {
    pub name: String,
    pub data: String,
    pub explicit_name: bool,
    pub created_seq: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BufferStore {
    buffers: Vec<PasteBuffer>,
    next_seq: u64,
    limit: usize,
}

impl Default for BufferStore {
    fn default() -> Self {
        Self {
            buffers: Vec::new(),
            next_seq: 1,
            limit: MAX_BUFFERS,
        }
    }
}

impl BufferStore {
    pub fn from_persisted(buffers: Vec<PasteBuffer>) -> Self {
        let next_seq = buffers
            .iter()
            .map(|buffer| buffer.created_seq)
            .max()
            .unwrap_or(0)
            + 1;
        Self {
            buffers,
            next_seq,
            ..Self::default()
        }
    }

    pub fn snapshot(&self) -> Vec<PasteBuffer> {
        self.buffers.clone()
    }

    pub fn top(&self) -> Option<&PasteBuffer> {
        self.buffers.first()
    }

    pub fn get(&self, name: Option<&str>) -> Option<&PasteBuffer> {
        match name {
            Some(name) => self.buffers.iter().find(|buffer| buffer.name == name),
            None => self.top(),
        }
    }

    pub fn set(
        &mut self,
        name: Option<String>,
        data: String,
        append: bool,
    ) -> Result<&PasteBuffer> {
        if data.len() > MAX_BUFFER_BYTES {
            bail!("paste buffer exceeds the {MAX_BUFFER_BYTES} byte limit");
        }
        if let Some(name) = name {
            if name.len() > 128 {
                bail!("paste buffer name exceeds the 128 byte limit");
            }
            if let Some(index) = self.buffers.iter().position(|buffer| buffer.name == name) {
                if append {
                    if self.buffers[index].data.len() + data.len() > MAX_BUFFER_BYTES {
                        bail!("paste buffer exceeds the {MAX_BUFFER_BYTES} byte limit");
                    }
                    self.buffers[index].data.push_str(&data);
                } else {
                    self.buffers[index].data = data;
                }
                self.buffers[index].explicit_name = true;
                return Ok(&self.buffers[index]);
            }
            let seq = self.next_sequence();
            self.buffers.insert(
                0,
                PasteBuffer {
                    name,
                    data,
                    explicit_name: true,
                    created_seq: seq,
                },
            );
            self.enforce_limit();
            return Ok(&self.buffers[0]);
        }

        let seq = self.next_sequence();
        let auto_name = format!("buffer{:04}", seq);
        self.buffers.insert(
            0,
            PasteBuffer {
                name: auto_name,
                data,
                explicit_name: false,
                created_seq: seq,
            },
        );
        self.enforce_limit();
        Ok(&self.buffers[0])
    }

    pub fn delete(&mut self, name: Option<&str>) -> Option<PasteBuffer> {
        let index = match name {
            Some(name) => self.buffers.iter().position(|buffer| buffer.name == name)?,
            None => (!self.buffers.is_empty()).then_some(0)?,
        };
        Some(self.buffers.remove(index))
    }

    pub fn summaries(&self) -> Vec<(String, usize, String)> {
        self.buffers
            .iter()
            .map(|buffer| {
                let sample = buffer
                    .data
                    .lines()
                    .next()
                    .unwrap_or("")
                    .chars()
                    .take(40)
                    .collect::<String>();
                (buffer.name.clone(), buffer.data.len(), sample)
            })
            .collect()
    }

    fn next_sequence(&mut self) -> u64 {
        loop {
            let seq = self.next_seq;
            self.next_seq = self.next_seq.wrapping_add(1);
            let name = format!("buffer{seq:04}");
            if !self.buffers.iter().any(|buffer| buffer.name == name) {
                return seq;
            }
        }
    }

    fn enforce_limit(&mut self) {
        if self.buffers.len() <= self.limit {
            return;
        }
        while self.buffers.len() > self.limit {
            self.buffers.pop();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auto_buffers_are_pushed_to_top() {
        let mut store = BufferStore::default();
        let first = store.set(None, "alpha".into(), false).expect("set").name.clone();
        let second = store.set(None, "beta".into(), false).expect("set").name.clone();
        assert_ne!(first, second);
        assert_eq!(store.top().expect("top").data, "beta");
    }

    #[test]
    fn explicit_buffers_are_updated_in_place() {
        let mut store = BufferStore::default();
        let _ = store.set(Some("named".into()), "alpha".into(), false).expect("set");
        let updated = store.set(Some("named".into()), "beta".into(), false).expect("set");
        assert_eq!(updated.data, "beta");
        assert_eq!(store.snapshot().len(), 1);
    }

    #[test]
    fn automatic_names_skip_existing_explicit_buffer_names() {
        let mut store = BufferStore::default();
        store.set(Some("buffer0002".into()), "explicit".into(), false).expect("set");
        store.set(None, "first".into(), false).expect("set");
        store.set(None, "second".into(), false).expect("set");

        let names: Vec<_> = store
            .snapshot()
            .into_iter()
            .map(|buffer| buffer.name)
            .collect();
        assert_eq!(names, vec!["buffer0004", "buffer0003", "buffer0002"]);
    }

    #[test]
    fn automatic_names_wrap_without_reusing_an_existing_name() {
        let mut store = BufferStore::default();
        store.next_seq = u64::MAX;
        assert_eq!(
            store.set(None, "last".into(), false).expect("set").name,
            "buffer18446744073709551615"
        );
        assert_eq!(store.set(None, "wrapped".into(), false).expect("set").name, "buffer0000");
    }

    #[test]
    fn deleting_without_name_removes_top_buffer() {
        let mut store = BufferStore::default();
        let _ = store.set(None, "alpha".into(), false).expect("set");
        let _ = store.set(None, "beta".into(), false).expect("set");
        let removed = store.delete(None).expect("delete top");
        assert_eq!(removed.data, "beta");
        assert_eq!(store.top().expect("remaining").data, "alpha");
    }

    #[test]
    fn explicit_buffers_obey_the_same_count_limit() {
        let mut store = BufferStore::default();
        for index in 0..=MAX_BUFFERS {
            store
                .set(Some(format!("named-{index}")), "data".into(), false)
                .expect("set explicit buffer");
        }
        assert_eq!(store.snapshot().len(), MAX_BUFFERS);
        assert!(store.get(Some("named-0")).is_none());
        assert!(store.get(Some("named-50")).is_some());
    }

    #[test]
    fn buffer_payloads_are_bounded_including_appends() {
        let mut store = BufferStore::default();
        assert!(store
            .set(None, "x".repeat(MAX_BUFFER_BYTES + 1), false)
            .is_err());
        store
            .set(Some("named".into()), "x".repeat(MAX_BUFFER_BYTES), false)
            .expect("set at limit");
        assert!(store.set(Some("named".into()), "x".into(), true).is_err());
    }
}
