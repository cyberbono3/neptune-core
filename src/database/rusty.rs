use super::leveldb::LevelDB;
use anyhow::Result;
use rusty_leveldb::DB;
use serde::{de::DeserializeOwned, Serialize};
use std::{
    marker::PhantomData,
    path::{Path, PathBuf},
};

pub struct RustyLevelDB<Key: Serialize + DeserializeOwned, Value: Serialize + DeserializeOwned> {
    database: DB,
    _key: PhantomData<Key>,
    _value: PhantomData<Value>,
}
// We have to implement `Debug` for `RustyLevelDB` as the `State` struct
// contains a database object, and `State` is used as input argument
// to multiple functions where logging is enabled with the `instrument`
// attributes from the `tracing` crate, and this requires all input
// arguments to the function to implement the `Debug` trait as this
// info is written on all logging events.
impl<Key: Serialize + DeserializeOwned, Value: Serialize + DeserializeOwned> core::fmt::Debug
    for RustyLevelDB<Key, Value>
{
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("").finish()
    }
}

// pub trait RustyDatabaseTable<Key: Serialize + DeserializeOwned, Value: Serialize + DeserializeOwned>:
// DatabaseTable<Key, Value>
impl<Key: Serialize + DeserializeOwned, Value: Serialize + DeserializeOwned> LevelDB<Key, Value>
    for RustyLevelDB<Key, Value>
{
    fn new<P: AsRef<Path>>(db_path: P, db_name: &str) -> Result<Self> {
        let mut path = PathBuf::new();
        path.push(db_path);
        path.push(db_name);
        let options = rusty_leveldb::Options::default();
        let db = DB::open(path, options)?;

        Ok(Self {
            database: db,
            _key: PhantomData,
            _value: PhantomData,
        })
    }

    fn get(&mut self, key: Key) -> Option<Value> {
        let key_bytes: Vec<u8> = bincode::serialize(&key).unwrap();
        let value_bytes: Option<Vec<u8>> = self.database.get(&key_bytes);
        value_bytes.map(|bytes| bincode::deserialize(&bytes).unwrap())
    }

    fn put(&mut self, key: Key, value: Value) {
        let key_bytes: Vec<u8> = bincode::serialize(&key).unwrap();
        let value_bytes: Vec<u8> = bincode::serialize(&value).unwrap();
        self.database.put(&key_bytes, &value_bytes).unwrap();
    }

    fn delete(&mut self, key: Key) -> Option<Value> {
        let key_bytes: Vec<u8> = bincode::serialize(&key).unwrap(); // add safety
        let value_bytes: Option<Vec<u8>> = self.database.get(&key_bytes);
        let value_object = value_bytes.map(|bytes| bincode::deserialize(&bytes).unwrap());
        let status = self.database.delete(&key_bytes);
        match status {
            Ok(_) => value_object, // could be None, if record is not present
            Err(err) => panic!("database failure: {}", err),
        }
    }
}
