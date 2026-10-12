//! A bounded sequential ownership model; focused tests cover concurrent schedules.

use std::time::Duration;

use rand::rngs::StdRng;
use rand::{RngExt, SeedableRng};
use testing::with_watchdog;

use super::*;
use crate::supervisor::ownership::{
    apply_client_message, attach_client, detach_client, stop_clients,
};
use crate::supervisor::relay::route_output;

/// One client and the complete ordered stream it may receive before abandonment.
struct ModelClient {
    server: ConnId,
    peer: ConnId,
    outbox: Arc<Outbox<MemoryTransport>>,
    expected: Vec<Message>,
    abandoned: bool,
}

#[test]
fn generated_ownership_and_opening_output_sequences() {
    // Fixed seeds and small operation/payload budgets exercise ordering under
    // Miri without timing-sensitive stress or unbounded worker creation.
    with_watchdog(|| {
        for seed in 0..2 {
            check_sequence(seed);
        }
    });
}

/// Applies a reproducible bounded operation stream to the model and real logic.
fn check_sequence(seed: u64) {
    let mut rng = StdRng::seed_from_u64(seed);
    let transport = MemoryTransport::new();
    let pty_host = MemoryPseudoconsole::new();
    let shared = shared_session(&transport, &pty_host);
    let mut clients: Vec<ModelClient> = Vec::new();
    let mut owner: Option<usize> = None;
    let mut opening = Vec::new();
    let mut claimed = false;
    let mut initiator_gone = false;
    let mut stopped = false;
    for step in 0..32 {
        match rng.random_range(0..6) {
            0 if !stopped => {
                let bytes = vec![rng.random(); rng.random_range(0..=32)];
                route_output(&shared, bytes.clone());
                if let Some(owner) = owner {
                    clients
                        .get_mut(owner)
                        .unwrap()
                        .expected
                        .push(Message::Output(bytes));
                } else if !claimed {
                    let keep = bytes
                        .len()
                        .min(MAX_CLIENT_BACKLOG_BYTES.checked_sub(opening.len()).unwrap());
                    opening.extend_from_slice(bytes.get(..keep).unwrap());
                }
            }
            1 => {
                let (server, peer) = connected_pair(&transport, &format!("client-{step}"));
                let generation = attach_client(&shared, server, DEFAULT_PTY_SIZE);
                if stopped {
                    assert!(generation.is_none());
                    assert_eq!(
                        transport
                            .recv_timeout(peer, Duration::ZERO)
                            .unwrap_err()
                            .kind(),
                        PalErrorKind::Disconnected
                    );
                    continue;
                }
                assert!(generation.is_some());
                if let Some(previous) = owner {
                    clients
                        .get_mut(previous)
                        .unwrap()
                        .expected
                        .push(Message::Displaced);
                }
                let mut expected = vec![Message::Attached {
                    session_id: shared.session_id,
                }];
                if !claimed {
                    expected.extend(preamble_messages(&opening));
                    opening.clear();
                }
                claimed = true;
                owner = Some(clients.len());
                clients.push(ModelClient {
                    server,
                    peer,
                    outbox: Arc::clone(&shared.client().as_ref().unwrap().outbox),
                    expected,
                    abandoned: false,
                });
            }
            2 if !clients.is_empty() => {
                let selected = rng.random_range(0..clients.len());
                let detached = detach_client(&shared, clients.get(selected).unwrap().server);
                assert_eq!(detached.is_some(), owner == Some(selected));
                if owner == Some(selected) {
                    clients.get_mut(selected).unwrap().abandoned = true;
                    owner = None;
                }
            }
            3 if !clients.is_empty() => {
                let selected = rng.random_range(0..clients.len());
                check_input(
                    &shared,
                    clients.get(selected).unwrap().server,
                    owner == Some(selected),
                    rng.random(),
                );
            }
            4 => {
                shared.note_initiator_gone();
                initiator_gone = true;
            }
            5 if step >= 24 => {
                let last = stop_clients(&shared);
                assert_eq!(
                    last.as_ref().map(|client| client.conn),
                    owner.map(|owner| clients.get(owner).unwrap().server)
                );
                if let Some(last) = last {
                    last.outbox.finish();
                }
                owner = None;
                stopped = true;
            }
            _ => {}
        }
        check_state(&shared, &clients, owner, claimed, initiator_gone, &opening);
    }
    check_deliveries(&shared, clients);
}

/// Compares ownership and first-attach retention after every generated operation.
fn check_state(
    shared: &Shared<MemoryTransport, MemoryPseudoconsole>,
    clients: &[ModelClient],
    owner: Option<usize>,
    claimed: bool,
    initiator_gone: bool,
    opening: &[u8],
) {
    assert_eq!(
        client_conn(shared),
        owner.map(|owner| clients.get(owner).unwrap().server)
    );
    let gate = shared.first_attach();
    assert_eq!(gate.claimed, claimed);
    assert_eq!(gate.initiator_gone, initiator_gone);
    drop(gate);
    let retained = shared.preamble.lock().unwrap();
    assert_eq!(
        retained.as_deref(),
        if claimed { None } else { Some(opening) }
    );
    assert!(
        retained
            .as_ref()
            .is_none_or(|bytes| bytes.len() <= MAX_CLIENT_BACKLOG_BYTES)
    );
}

/// Retires every writer before comparing its bounded, immediately available stream.
fn check_deliveries(
    shared: &Shared<MemoryTransport, MemoryPseudoconsole>,
    clients: Vec<ModelClient>,
) {
    if let Some(last) = stop_clients(shared) {
        last.outbox.finish();
    }
    for client in clients {
        let outbox = client.outbox;
        outbox.finish();
        outbox.wait_for_writer();
        let mut received = Vec::new();
        while let Ok(message) = shared.transport.recv_timeout(client.peer, Duration::ZERO) {
            assert!(
                received.len() < client.expected.len(),
                "unexpected output or replay"
            );
            received.push(message);
        }
        if client.abandoned {
            assert!(client.expected.starts_with(&received));
        } else {
            assert_eq!(received, client.expected);
        }
    }
}

/// Checks both input and geometry against the model's sole-owner decision.
fn check_input(
    shared: &Shared<MemoryTransport, MemoryPseudoconsole>,
    server: ConnId,
    owns: bool,
    byte: u8,
) {
    let input = vec![byte];
    let accepted = apply_client_message(shared, server, Message::Input(input.clone()));
    assert_eq!(accepted, owns);
    assert_eq!(
        shared.pty_host.take_input(shared.pty),
        if owns { input } else { Vec::new() }
    );
    // Arbitrary nondefault, unequal axes make applying an unauthorized resize visible.
    let size = WindowSize::new(91, 37).unwrap();
    let before = shared.pty_host.size(shared.pty);
    assert_eq!(
        apply_client_message(shared, server, Message::Resize { size }),
        owns
    );
    assert_eq!(
        shared.pty_host.size(shared.pty),
        if owns { Some(size) } else { before }
    );
}
