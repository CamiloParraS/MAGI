use super::*;
use std::sync::Mutex;

#[derive(Default)]
struct Recorder(Mutex<Vec<&'static str>>);

impl TextEmbedder for Recorder {
    fn model_id(&self) -> &str {
        "recorder"
    }
    fn dim(&self) -> usize {
        1
    }
    fn embed_passages(&self, texts: &[&str]) -> magi_core::Result<Vec<Vec<f32>>> {
        Ok(texts.iter().map(|_| vec![0.0]).collect())
    }
    fn embed_query(&self, _text: &str) -> magi_core::Result<Vec<f32>> {
        self.0.lock().unwrap().push("embed");
        Ok(vec![1.0])
    }
    fn unload_if_idle(&self, idle: Duration) {
        assert_eq!(idle, Duration::ZERO);
        self.0.lock().unwrap().push("unload");
    }
}

#[test]
fn one_shot_embedder_unloads_right_after_the_query() {
    let inner = Recorder::default();
    let v = OneShotQuery(&inner).embed_query("q").unwrap();
    assert_eq!(v, vec![1.0]);
    assert_eq!(*inner.0.lock().unwrap(), vec!["embed", "unload"]);
}
