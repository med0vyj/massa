// Copyright (c) 2023 MASSA LABS <info@massa.net>

use crate::error::{match_for_io_error, GrpcError};
use crate::server::MassaGrpc;
use futures_util::StreamExt;
use massa_proto_rs::massa::api::v1 as grpc_api;
use std::io::ErrorKind;
use std::pin::Pin;
use tokio::select;
use tonic::codegen::futures_core;
use tonic::{Request, Streaming};
use tracing::log::{error, warn};

/// Type declaration for NewBlocksHeaders
pub type NewBlocksHeadersStreamType = Pin<
    Box<
        dyn futures_core::Stream<Item = Result<grpc_api::NewBlocksHeadersResponse, tonic::Status>>
            + Send
            + 'static,
    >,
>;

/// Creates a new stream of new produced and received blocks headers
pub(crate) async fn new_blocks_headers(
    grpc: &MassaGrpc,
    request: Request<Streaming<grpc_api::NewBlocksHeadersRequest>>,
) -> Result<NewBlocksHeadersStreamType, GrpcError> {
    // Create a channel to handle communication with the client
    let (tx, rx) = tokio::sync::mpsc::channel(grpc.grpc_config.max_channel_size);
    // Get the inner stream from the request
    let mut in_stream = request.into_inner();
    // Subscribe to the new blocks headers channel
    let mut subscriber = grpc.consensus_channels.block_header_sender.subscribe();

    tokio::spawn(async move {
        // Initialize the request_id string
        let mut request_id = String::new();
        loop {
            select! {
                // Receive a new block header from the subscriber
                 event = subscriber.recv() => {
                    match event {
                        Ok(massa_block_header) => {
                            // Send the new block header through the channel
                            if let Err(e) = tx.send(Ok(grpc_api::NewBlocksHeadersResponse {
                                    id: request_id.clone(),
                                    block_header: Some(massa_block_header.into())
                            })).await {
                                error!("failed to send new block header : {}", e);
                                break;
                            }
                        },
                        Err(e) => error!("error on receive new block header : {}", e)
                    }
                },
            // Receive a new message from the in_stream
            res = in_stream.next() => {
                match res {
                    Some(res) => {
                        match res {
                            // Get the request_id from the received data
                            Ok(data) => {
                                request_id = data.id
                            },
                            // Handle any errors that may occur during receiving the data
                            Err(err) => {
                                // Check if the error matches any IO errors
                                if let Some(io_err) = match_for_io_error(&err) {
                                    if io_err.kind() == ErrorKind::BrokenPipe {
                                        warn!("client disconnected, broken pipe: {}", io_err);
                                        break;
                                    }
                                }
                                error!("{}", err);
                                // Send the error response back to the client
                                if let Err(e) = tx.send(Err(err)).await {
                                    error!("failed to send back new_blocks_headers error response: {}", e);
                                    break;
                                }
                            }
                        }
                    },
                    None => {
                        // The client has disconnected
                        break;
                    },
                }
            }
            }
        }
    });

    let out_stream = tokio_stream::wrappers::ReceiverStream::new(rx);
    Ok(Box::pin(out_stream) as NewBlocksHeadersStreamType)
}
