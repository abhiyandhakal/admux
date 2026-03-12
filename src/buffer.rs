use serde::{Deserialize, Serialize};

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
            limit: 50,
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

    pub fn set(&mut self, name: Option<String>, data: String, append: bool) -> &PasteBuffer {
        if let Some(name) = name {
            if let Some(index) = self.buffers.iter().position(|buffer| buffer.name == name) {
                if append {
                    self.buffers[index].data.push_str(&data);
                } else {
                    self.buffers[index].data = data;
                }
                self.buffers[index].explicit_name = true;
                return &self.buffers[index];
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
            return &self.buffers[0];
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
        &self.buffers[0]
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
        let seq = self.next_seq;
        self.next_seq += 1;
        seq
    }

    fn enforce_limit(&mut self) {
        let mut auto_count = self
            .buffers
            .iter()
            .filter(|buffer| !buffer.explicit_name)
            .count();
        if auto_count <= self.limit {
            return;
        }
        while auto_count > self.limit {
            if let Some(index) = self
                .buffers
                .iter()
                .rposition(|buffer| !buffer.explicit_name)
            {
                self.buffers.remove(index);
                auto_count -= 1;
            } else {
                break;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auto_buffers_are_pushed_to_top() {
        let mut store = BufferStore::default();
        let first = store.set(None, "alpha".into(), false).name.clone();
        let second = store.set(None, "beta".into(), false).name.clone();
        assert_ne!(first, second);
        assert_eq!(store.top().expect("top").data, "beta");
    }

    #[test]
    fn explicit_buffers_are_updated_in_place() {
        let mut store = BufferStore::default();
        let _ = store.set(Some("named".into()), "alpha".into(), false);
        let updated = store.set(Some("named".into()), "beta".into(), false);
        assert_eq!(updated.data, "beta");
        assert_eq!(store.snapshot().len(), 1);
    }

    #[test]
    fn deleting_without_name_removes_top_buffer() {
        let mut store = BufferStore::default();
        let _ = store.set(None, "alpha".into(), false);
        let _ = store.set(None, "beta".into(), false);
        let removed = store.delete(None).expect("delete top");
        assert_eq!(removed.data, "beta");
        assert_eq!(store.top().expect("remaining").data, "alpha");
    }
}
