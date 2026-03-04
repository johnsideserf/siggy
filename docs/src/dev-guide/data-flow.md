# Data Flow

## Outbound messages (user sends)

```
1. User types message + presses Enter
2. input::parse_input() -> InputAction::SendText
3. App sends JsonRpcRequest via mpsc to SignalClient
4. SignalClient writes JSON-RPC to signal-cli stdin
5. signal-cli transmits via Signal protocol
```

The request is a JSON-RPC call to the `send` method with the recipient and
message body as parameters. Each request gets a unique UUID as its RPC ID.

## Inbound messages (received)

```
1. signal-cli receives message via Signal protocol
2. signal-cli writes JSON-RPC notification to stdout
3. SignalClient stdout reader parses the JSON line
4. Notification has method = "receive" with message data
5. Parsed into SignalEvent::MessageReceived
6. Sent through mpsc channel to main thread
7. App::handle_signal_event() processes it:
   a. get_or_create_conversation() ensures the conversation exists
   b. Message is appended to the conversation's message list
   c. Message is inserted into SQLite
   d. Unread count is updated (if not the active conversation)
   e. Terminal bell fires (if notifications enabled and not muted)
8. Next render cycle shows the new message
```

## RPC request/response correlation

signal-cli uses JSON-RPC 2.0. There are two types of messages:

### Notifications (incoming)

Notifications arrive as JSON-RPC **requests** from signal-cli (they have a
`method` field). These include:

- `receive` -- incoming message
- `receiveTyping` -- typing indicator
- `receiveReceipt` -- delivery/read receipt

These are unsolicited and do not have an `id` field matching any outbound request.

### RPC responses

When siggy sends a request (e.g., `listContacts`, `listGroups`, `send`),
signal-cli replies with a response that has a matching `id` field and a `result`
(or `error`) field.

The `pending_requests` map in `SignalClient` stores `id -> method` pairs. When
a response arrives, the client looks up the method by ID to know how to parse
the result:

```
outbound:  { "jsonrpc": "2.0", "id": "abc-123", "method": "listContacts", ... }
inbound:   { "jsonrpc": "2.0", "id": "abc-123", "result": [...] }

pending_requests["abc-123"] = "listContacts"
-> parse result as Vec<Contact>
-> emit SignalEvent::ContactList
```

## Sync messages

When you send a message from your phone, signal-cli receives a sync notification.
These appear as `SignalMessage` with `is_outgoing = true` and a `destination`
field indicating the recipient. The app routes these to the correct conversation
and displays them as outgoing messages.

## Channel architecture

```
                     mpsc::channel
SignalClient ───────────────────────> App (main thread)
  (tokio tasks)      SignalEvent

                     mpsc::channel
App (main thread) ──────────────────> SignalClient
                     JsonRpcRequest     (stdin writer task)
```

Both channels are unbounded `tokio::sync::mpsc` channels. The signal event
channel carries `SignalEvent` variants. The command channel carries
`JsonRpcRequest` structs to be serialized and written to signal-cli's stdin.
