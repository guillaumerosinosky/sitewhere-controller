use std::{env,fs};
use std::io::{Error, ErrorKind, Bytes};
use reqwest::{header::HeaderMap, Response, Error as ReqwestError, get};
use tracing::{info, error, warn, debug};
use std::collections::HashMap;
use serde_json::{json, Value};
use std::time::{Duration, SystemTime};
use futures::{stream::StreamExt,};
use tokio::sync::mpsc;
use rumqttc::{MqttOptions, AsyncClient, QoS, EventLoop, Event, Incoming, Packet, ConnectionError};

use s3::error::S3Error;
use s3::{Bucket};
use s3::creds::Credentials;
use s3::Region;

#[derive(Debug)]
//#[derive(Clone)]
pub struct SitewhereHttpClient {
    api_gateway_url: String,
    headers: Option<HeaderMap>,
    devices: Option<serde_json::Value>,
    assignments: Option<serde_json::Value>,
    device_types: Option<serde_json::Value>,
    commands: Option<serde_json::Value>,

    map_device_types: HashMap<String, String>,
    map_assignments: HashMap<String, String>,
    map_commands: HashMap<String, String>,
    map_devices: HashMap<String, (String, String)>,

    //mqtt_receiver: AsyncReceiver<Option<Message>>,
    mqtt_receiver: mpsc::UnboundedReceiver<Option<(u128, Vec<u8>)>>,
    sender_results: mpsc::UnboundedSender<String>,
}

fn get_now_us() -> u128 {
    let duration_since_epoch = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).unwrap();
    let timestamp_nanos = duration_since_epoch.as_micros(); // u128
    timestamp_nanos
}

impl SitewhereHttpClient {
    async fn login(&mut self, client:reqwest::Client) -> Result<(), ReqwestError> {
        let resp = client.get(format!("{}{}", self.api_gateway_url, "authapi/jwt"))
            .basic_auth("admin", Some("password"))
            .send()
            .await?;
        //println!("{:?}", resp);
        let mut headers = HeaderMap::new();    
        let jwt = resp.headers().get("x-sitewhere-jwt").unwrap().to_str().unwrap();
        headers.insert("Authorization", (format!("Bearer {}", jwt)).parse().unwrap());
        headers.insert("X-SiteWhere-Tenant-Id", "default".parse().unwrap());
        headers.insert("X-SiteWhere-Tenant-Auth", "sitewhere1234567890".parse().unwrap());
        headers.insert("Content-Type", "application/json;charset=UTF-8".parse().unwrap());
        self.headers = Some(headers);
        Ok(())
    }
    
    async fn query(& self, client:reqwest::Client, path: &str) -> Result<serde_json::Value, ReqwestError> {
        let resp: serde_json::Value = client.get(format!("{}{}", self.api_gateway_url, path))
        .headers(self.headers.clone().unwrap())
        .send()
        .await?
        .json()
        .await?;

        Ok(resp)
    }

    async fn send_command_payload(&self, client:reqwest::Client, command_id:String, assignment_token: String, token:String, parameter_values: Value, metadata: Value) -> Result<(), ReqwestError> {
        let payload = json!({
            "commandToken": command_id,
            "initiator": "REST",
            "initiatorId": "admin",
            "metadata": metadata,
            "parameterValues": parameter_values,
            "target": "Assignment",
            "eventDate": get_now_us().to_string(),
            "targetId": token,
            "updateState": true      
        });

        debug!("sending command {}", payload);
        let resp = client.post(format!("{}{}", self.api_gateway_url, format!("api/assignments/{}/invocations", assignment_token)))
            .headers(self.headers.clone().unwrap())
            .body(payload.to_string())
            .send()
            .await?;
        debug!("sent command {:?}", resp);
        Ok({})
    }

    async fn init(&mut self, client:reqwest::Client) -> Result<(), ReqwestError> {
        info!("Init SitewhereHttpClient");
        self.login(client.clone()).await?;

        self.devices = Some(self.query(client.clone(), "api/devices?pageSize=1000").await?["results"].clone());
        info!("Loaded {:?} devices", self.devices.as_ref().unwrap().as_array().unwrap().len());
        self.device_types = Some(self.query(client.clone(), "api/devicetypes?pageSize=1000").await?["results"].clone());
        info!("Loaded {} device types", self.device_types.as_ref().unwrap().as_array().unwrap().len());
        self.commands = Some(self.query(client.clone(), "api/commands?pageSize=1000").await?["results"].clone());
        info!("Loaded {} commands", self.commands.as_ref().unwrap().as_array().unwrap().len());
        self.assignments = Some(self.query(client.clone(), "api/assignments?pageSize=1000").await?["results"].clone());
        info!("Loaded {} assignments", self.assignments.as_ref().unwrap().as_array().unwrap().len());

        if let Some(elements) = self.device_types.as_ref().unwrap().as_array() {
            for element in elements {
                self.map_device_types.insert(element["id"].as_str().unwrap().to_string(), element["token"].as_str().unwrap().to_string());
            }
        }

        if let Some(elements) = self.assignments.as_ref().unwrap().as_array() {
            for element in elements {
                self.map_assignments.insert(element["id"].as_str().unwrap().to_string(), element["token"].as_str().unwrap().to_string());
            }
        }

        if let Some(elements) = self.commands.as_ref().unwrap().as_array() {
            for element in elements {
                if element["name"] == "ping" {
                    self.map_commands.insert(element["deviceTypeId"].as_str().unwrap().to_string(), element["token"].as_str().unwrap().to_string());
                }
                
            }
        }

        if let Some(elements) = self.devices.as_ref().unwrap().as_array() {
            for element in elements {
                self.map_devices.insert(element["id"].as_str().unwrap().to_string(), (element["token"].as_str().unwrap().to_string(), element["deviceTypeId"].as_str().unwrap().to_string()));
            }
        }
        Ok({})
    }

    async fn loop_message(& mut self, client:reqwest::Client) -> Result<(), ReqwestError> {
        info!("http loop message start");
        //let mut receiver = self.mqtt_receiver.clone();
        while let Some(msg_opt) = self.mqtt_receiver.recv().await {
            //let Some(now, msg) = msg_opt;
            //let payload = ;
            if msg_opt.is_none() {
                continue
            }
            let (now, msg_opt) = msg_opt.unwrap();
            let payload = msg_opt;
            if payload.eq(b"STOP") {
                break   
            }            
            let decoded_message: Value = serde_json::from_slice(&payload).unwrap();
            debug!("Received message: {}", decoded_message);
            

            //let event_date: u128 = decoded_message["eventDate"].as
            
            let event_date: u128 = decoded_message["eventDate"].to_string().trim().parse().unwrap();
            //let event_date: serde_json::Number = decoded_message["eventDate"];
            let event_date: u128 = event_date.to_string().parse().unwrap();
            //let event_date: u128 = decoded_message["eventDate"].as_str().unwrap_or("0").trim().parse().unwrap();
            let mut received_date: u128 = decoded_message["receivedDate"].to_string().trim().parse().unwrap();
            received_date = received_date * 1000;
            let event_type = decoded_message["eventType"].as_str();
            match &event_type {
                Some("Alert") => {
                    let metadata = json!({
                        "precision": "mu",
                        "originEventDate": event_date
                    });
                    let parameter_values = json!({
                        
                    });
                    let (_device_id, device_type_id) = self.map_devices[&decoded_message["deviceId"].as_str().unwrap().to_string()].clone();
                    let assignment_token = self.map_assignments[&decoded_message["deviceAssignmentId"].as_str().unwrap().to_string()].clone();
                    let command_id = self.map_commands[&device_type_id].clone();
                    debug!("Command id: {}", command_id);
                    let cli = client.clone();
                    // send command
                    self.send_command_payload(
                        cli,
                        command_id,
                        assignment_token.clone(),
                        assignment_token,
                        parameter_values,
                        metadata
                        ).await?;
                },
                Some(event_type) => {
                    debug!("Received {:?}", event_type);
                },
                None => {
                    error!("unable to unpack eventType {:?}", decoded_message);
                }
            };
            let device_id:String = decoded_message["deviceId"].as_str().unwrap().to_string();
            let (device_token, _) = &self.map_devices[&device_id];
            let line_result = format!("{},{},{},{},{}",
                device_token.clone(),
                event_type.unwrap_or("None"),
                event_date,
                received_date,
                now);
            debug!("Line result: {} {} {} {}", &line_result, now - event_date, received_date - event_date, now - received_date);
            //self.results.push(line_result);
            self.sender_results.send(line_result);

        }
        self.sender_results.send("".to_string());
        info!("http loop message end");
        //self.write_results().await;
        Ok(())
    }


}

#[derive(Debug)]
#[derive(Clone)]
struct SitewhereMqttClient {
    broker_url: String,
    broker_port: u16,
    topic: String,
    qos: i32,
    buffer_size: i32,
    sender_mqtt: mpsc::UnboundedSender<Option<(u128, Vec<u8>)>>,
}

impl SitewhereMqttClient {
    async fn init(&mut self) -> Result<(rumqttc::AsyncClient, rumqttc::EventLoop), ConnectionError> {
        info!("Initializing MQTT client {:?}", self);
        let mut mqttoptions = MqttOptions::new("controller-consumer", self.broker_url.clone(), self.broker_port);
        mqttoptions.set_keep_alive(Duration::from_secs(600));

        let (mut client, mut eventloop) = AsyncClient::new(mqttoptions, 10000);
        
        Ok((client.clone(), eventloop))
    }

    async fn loop_message(&mut self, client:AsyncClient, mut msg_stream:EventLoop) -> Result<(()), ConnectionError> {
        client.subscribe(self.topic.clone(), QoS::AtMostOnce).await.unwrap();
        info!("mqtt subscribed");
        
        let sender = self.sender_mqtt.clone();
        tokio::spawn(async move{
            
            while let event = msg_stream.poll().await{
                /*
                if !client.is_connected() {
                    warn!("Client is not connected, attempting reconnect");
                    match client.reconnect().await {
                        Ok(result) => warn!("Successfully reconnected: {:?}", result),
                        Err(error) => 
                        {
                            error!("Failed to reconnect: {:?}", error)
                        },
                    }
                } */
                match event {
                    Ok(Event::Incoming(Packet::Publish(msg))) => {
                        let msg = Some((get_now_us(), msg.payload.to_vec()));
                        sender.send(msg);
                    },
                    Err(e) => {
                        error!("error: {:?}", e);
                        break
                    }
                    event => {
                        debug!("Received {:?}", event);
                    }
                };
                
                //tokio::time::sleep(Duration::from_secs(1)).await;
            }
            //Ok({})
        }).await;
        /*
        while let Some(msg_opt) = msg_stream.next().await {
            if let Some(msg) = msg_opt {
                println!("message (mqtt) {}", msg);
            } else {
                println!("nop");
            }            
        } */

        Ok(())
    }
}

#[tracing::instrument]
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    if env::var_os("DEBUG").is_some() {
        tracing_subscriber::fmt().with_max_level(tracing::Level::DEBUG).try_init().unwrap();
    } else {
        tracing_subscriber::fmt().with_max_level(tracing::Level::INFO).try_init().unwrap();
    }
    let api_gateway_address = env::var("API_GATEWAY_ADDRESS").unwrap_or("20.74.29.222".to_string());
    let api_gateway_url = format!("http://{}/sitewhere/", api_gateway_address);
    let broker_address = env::var("BROKER_ADDRESS").unwrap_or("20.74.25.142".to_string());
    let broker_port: u16 = env::var("BROKER_PORT").unwrap_or("1883".to_string()).parse().unwrap_or(1883);
    //let broker_url = format!("tcp://{}:{}", broker_address, broker_port);
    
    let minio_address = env::var("MINIO_ADDRESS").unwrap_or("http://20.19.22.71:9000".into());
    let minio_access_key = env::var("MINIO_ACCESS_KEY").unwrap_or("admin".into());
    let minio_secret_key = env::var("MINIO_SECRET_KEY").unwrap_or("6L320jOQwm".into());
    let minio_bucket_name = env::var("MINIO_BUCKET").unwrap_or("results".into());
    let xp_name = env::var("XP_NAME").unwrap_or("test".into());
    let topic = env::var("TOPIC").unwrap_or("SiteWhere/default/output/mqtt1".into());

    info!("Checking Minio availability");
    let bucket = Bucket::new(
        &minio_bucket_name,
        Region::Custom {
            region: "".to_owned(),
            endpoint: minio_address,
        },
        Credentials::new(
            Some(&minio_access_key),
            Some(&minio_secret_key),
            None,
            None,
            None
        )?
    )?
    .with_path_style();

    let test = bucket.list("/".to_string(), None).await?;
    info!("Results: {:?}", test);
    info!("Launching Sitewhere controller...");

    let (sender_results, mut receiver_results) =  mpsc::unbounded_channel::<String>();
    let (sender_mqtt, mut receiver_mqtt) =  mpsc::unbounded_channel();

    let mut sitewhere_mqtt_client = SitewhereMqttClient {
        broker_url: broker_address,
        broker_port: broker_port,
        topic: topic.to_string(),
        qos: 1,
        buffer_size: 10000,
        sender_mqtt: sender_mqtt,
    };
    let (mqtt_client, mqtt_receiver) = sitewhere_mqtt_client.init().await?;

    let client = reqwest::Client::new();

    let mut sitewhere_http_client = SitewhereHttpClient {
        api_gateway_url: api_gateway_url.clone(),
        headers: None,
        assignments:None,
        commands: None,
        device_types: None,
        devices: None,     
        map_assignments: HashMap::new(),
        map_commands: HashMap::new(),
        map_device_types: HashMap::new(),
        map_devices: HashMap::new(),
        mqtt_receiver: receiver_mqtt,
        sender_results: sender_results.clone(),
    };

    sitewhere_http_client.init(client.clone()).await?;

    let mqtt_worker = async move {
        match sitewhere_mqtt_client.loop_message(mqtt_client, mqtt_receiver).await {
            Ok(v) => {
                info!("mqtt worker finished");
                Ok(v)
            },
            Err(e) => {
                error!("mqtt worker {:?}", e);
                Err(Error::new(ErrorKind::Other, "mqtt error"))
            }
        }
    };

    let http_worker = async move {
        match sitewhere_http_client.loop_message(client).await {
            Ok(v) => {
                info!("http worker finished");
                //Err(Error::new(ErrorKind::Other, "http error"))
                Ok(v)
            },
            Err(e) => {
                error!("http worker {:?}", e);
                Err(Error::new(ErrorKind::Other, "http error"))
            }
        }
        
    };


    let mut results = Vec::new();
    let results_grabber = async move {
        while let Some(result) = receiver_results.recv().await {
            if result == "" {
                break
            }
            results.push(result);
        }
        fs::write("./results.csv", results.clone().join("\n")).expect("Unable to write file");

        match bucket.put_object(format!("{}/{}", xp_name, "controller.csv"), results.join("\n").as_bytes()).await {
            Ok(_v) => {
                info!("Written {} results on {}", results.len(), xp_name + "/controller.csv");
            }
            Err(e) => {
                panic!("Error while writing results on Minio: {}", e);
            }
        };

        Ok({})
    };

    futures::try_join!(mqtt_worker, http_worker, results_grabber);


    info!("Finished.");


    Ok(())
}