# Data Flow

## Outbound messages (user sends)

```mermaid
sequenceDiagram
    participant U as User
    participant I as input.rs
    participant A as App
    participant SC as SignalClient
    participant CLI as signal-cli

    U->>I: Type message + Enter
    I->>A: InputAction::SendText
    A->>A: Add message locally<br/>(optimistic display)
    A->>A: Persist to SQLite
    A->>SC: JsonRpcRequest (mpsc)
    SC->>CLI: JSON-RPC via stdin
    CLI-->>SC: Response with server timestamp
    SC-->>A: SignalEvent::SendResult
    A->>A: Update message status<br/>(Sending → Sent)
```

The request is a JSON-RPC call to the `send` method with the recipient and
message body as parameters. Each request gets a unique UUID as its RPC ID.

## Inbound messages (received)

```mermaid
sequenceDiagram
    participant CLI as signal-cli
    participant SR as stdout reader
    participant A as App
    participant DB as SQLite
    participant UI as ui/

    CLI->>SR: JSON-RPC notification<br/>(method: "receive")
    SR->>A: SignalEvent::MessageReceived<br/>(mpsc channel)
    A->>A: get_or_create_conversation()
    A->>A: Append to message list
    A->>DB: Insert message
    A->>A: Update unread count
    A->>A: Reorder sidebar
    Note over A: Terminal bell<br/>(if enabled + not muted)
    A->>UI: Next render cycle
```

## RPC request/response correlation

signal-cli uses JSON-RPC 2.0. There are two types of messages:

### Notifications (incoming)

Notifications arrive as JSON-RPC **requests** from signal-cli (they have a
`method` field). These include:

- `receive` - incoming message
- `receiveTyping` - typing indicator
- `receiveReceipt` - delivery/read receipt

These are unsolicited and do not have an `id` field matching any outbound request.

### RPC responses

When siggy sends a request (e.g., `listContacts`, `listGroups`, `send`),
signal-cli replies with a response that has a matching `id` field and a `result`
(or `error`) field.

```mermaid
sequenceDiagram
    participant S as siggy
    participant CLI as signal-cli

    S->>CLI: {"id": "abc-123",<br/>"method": "listContacts"}
    Note over S: pending_requests["abc-123"]<br/>= "listContacts"
    CLI-->>S: {"id": "abc-123",<br/>"result": [...]}
    Note over S: Lookup method by ID<br/>→ parse as Vec<Contact><br/>→ emit ContactList
```

The `pending_requests` map in `SignalClient` stores `id → method` pairs. When
a response arrives, the client looks up the method by ID to know how to parse
the result.

## Sync messages

When you send a message from your phone, signal-cli receives a sync notification.
These appear as `SignalMessage` with `is_outgoing = true` and a `destination`
field indicating the recipient. The app routes these to the correct conversation
and displays them as outgoing messages.

## Channel architecture

```mermaid
graph LR
    subgraph tokio["SignalClient (tokio tasks)"]
        SR["stdout reader"]
        SW["stdin writer"]
    end

    subgraph main["App (main thread)"]
        APP["App state"]
    end

    SR -- "SignalEvent<br/>(mpsc, unbounded)" --> APP
    APP -- "JsonRpcRequest<br/>(mpsc, unbounded)" --> SW
```

Both channels are unbounded `tokio::sync::mpsc` channels. The signal event
channel carries `SignalEvent` variants. The command channel carries
`JsonRpcRequest` structs to be serialized and written to signal-cli's stdin.
