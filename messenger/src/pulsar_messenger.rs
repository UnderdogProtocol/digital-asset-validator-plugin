#![cfg(feature = "pulsar")]

use {
    crate::{error::MessengerError, Messenger, MessengerConfig},
    async_mutex::Mutex,
    async_trait::async_trait,
    futures::TryStreamExt,
    log::*,
    pulsar::{Authentication, Consumer, Producer, Pulsar, TokioExecutor},
    std::sync::Arc,
    std::{
        collections::HashMap,
        fmt::{Debug, Formatter},
    },
};

#[derive(Default)]
pub struct PulsarMessenger {
    connection: Option<Pulsar<TokioExecutor>>,
    producers: HashMap<&'static str, Producer<TokioExecutor>>,
    consumers: HashMap<&'static str, Arc<Mutex<Consumer<Vec<u8>, TokioExecutor>>>>,
    max_buffer_size: HashMap<&'static str, usize>,
}

const PULSAR_CON_STR: &str = "pulsar_connection_str";
const PULSAR_AUTH_TOKEN: &str = "pulsar_auth_token";

#[async_trait]
impl Messenger for PulsarMessenger {
    /// Create new Pulsar connection for future topics
    async fn new(config: MessengerConfig) -> Result<Self, MessengerError> {
        let uri = config
            .get(&*PULSAR_CON_STR)
            .and_then(|u| u.clone().into_string())
            .ok_or(MessengerError::ConfigurationError {
                msg: format!("Connection String Missing: {}", PULSAR_CON_STR),
            })?;

        let mut builder = Pulsar::builder(uri, TokioExecutor);

        if let Some(token) = config.get(&*PULSAR_AUTH_TOKEN) {
            let authentication = Authentication {
                name: String::from("token"),
                data: token.clone().into_string().unwrap().into_bytes(),
            };
            builder = builder.with_auth(authentication);
        }

        let pulsar: Pulsar<_> = builder.build().await.unwrap();

        Ok(Self {
            connection: Some(pulsar),
            producers: HashMap::<&'static str, Producer<TokioExecutor>>::default(),
            consumers:
                HashMap::<&'static str, Arc<Mutex<Consumer<Vec<u8>, TokioExecutor>>>>::default(),
            max_buffer_size: HashMap::<&'static str, usize>::default(),
        })
    }

    /// Create new Producer for Pulsar topic
    async fn add_stream(&mut self, stream_key: &'static str) {
        if self.producers.contains_key(stream_key) {
            error!("Stream {stream_key} already exists");
            return;
        }

        let producer = self
            .connection
            .as_mut()
            .unwrap()
            .producer()
            .with_topic(stream_key)
            .build()
            .await
            .unwrap();

        self.producers.insert(stream_key, producer);

        if self.consumers.contains_key(stream_key) {
            error!("Consumer for {stream_key} already exists");
            return;
        }

        let consumer: Consumer<Vec<u8>, _> = self
            .connection
            .as_mut()
            .unwrap()
            .consumer()
            .with_topic(stream_key)
            .build()
            .await
            .unwrap();

        self.consumers
            .insert(stream_key, Arc::new(Mutex::new(consumer)));
    }

    /// Set max buffer size for the stream
    async fn set_buffer_size(&mut self, stream_key: &'static str, max_buffer_size: usize) {
        if self.max_buffer_size.contains_key(stream_key) {
            error!("Max buffer size already set for {stream_key} topic");
            return;
        }

        self.max_buffer_size.insert(stream_key, max_buffer_size);
    }

    /// Send message to the Pulsar topic
    async fn send(&mut self, stream_key: &'static str, bytes: &[u8]) -> Result<(), MessengerError> {
        if let Some(max_buffer_size) = self.max_buffer_size.get(stream_key) {
            if bytes.len() > *max_buffer_size {
                error!("Cannot send data for topic {stream_key}, buffer size is exaggerated");
                return Err(MessengerError::SendError {
                    msg: String::from("Buffer size is exaggerated"),
                });
            }
        }

        // Check if topic is configured
        let producer = if let Some(producer) = self.producers.get_mut(stream_key) {
            producer
        } else {
            error!("Cannot send data for topic {stream_key}, it is not configured");
            return Ok(());
        };

        let result = producer.send(bytes).await.unwrap().await;

        if let Err(e) = result {
            error!("Pulsar send error: {e}");
            return Err(MessengerError::SendError { msg: e.to_string() });
        } else {
            info!("Data sent");
        }

        Ok(())
    }

    async fn recv(
        &mut self,
        stream_key: &'static str,
    ) -> Result<Vec<(i64, Vec<u8>)>, MessengerError> {
        // Check if consumer is exists
        let mut consumer = if let Some(consumer) = self.consumers.get(stream_key) {
            consumer.lock().await
        } else {
            error!("Cannot get data from topic {stream_key}, consumer is not configured");
            return Err(MessengerError::ReceiveError {
                msg: String::from("Consumer for the requested topic wasn't created"),
            });
        };

        let result = consumer.try_next().await.unwrap();

        if let Some(msg) = result {
            consumer.ack(&msg).await.unwrap();
            let data = msg.deserialize();
            return Ok(vec![(0, data)]); // TODO: it is not universal data type
        } else {
            return Err(MessengerError::ReceiveError {
                msg: String::from("No data in requested topic found"),
            });
        }
    }
}

impl Debug for PulsarMessenger {
    fn fmt(&self, _f: &mut Formatter<'_>) -> std::fmt::Result {
        Ok(())
    }
}
