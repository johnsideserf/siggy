# signal-cli Protocol

siggy communicates with signal-cli using
[JSON-RPC 2.0](https://www.jsonrpc.org/specification) over stdin/stdout. signal-cli
is spawned as a child process in `jsonRpc` mode.

## Starting signal-cli

signal-cli is launched with:

```sh
signal-cli -a +15551234567 jsonRpc
```

This starts signal-cli in JSON-RPC mode, reading requests from stdin and writing
responses/notifications to stdout. Each message is a single JSON line.

## Request format

Requests sent from siggy to signal-cli:

```json
{
    "jsonrpc": "2.0",
    "id": "550e8400-e29b-41d4-a716-446655440000",
    "method": "send",
    "params": {
        "recipient": ["+15551234567"],
        "message": "Hello!"
    }
}
```

Each request has a unique UUID `id` for response correlation.

## Response format

Responses from signal-cli for RPC calls:

```json
{
    "jsonrpc": "2.0",
    "id": "550e8400-e29b-41d4-a716-446655440000",
    "result": { ... }
}
```

Or on error:

```json
{
    "jsonrpc": "2.0",
    "id": "550e8400-e29b-41d4-a716-446655440000",
    "error": {
        "code": -1,
        "message": "error description"
    }
}
```

## Notification format

Notifications are unsolicited JSON-RPC requests from signal-cli (no matching
outbound request). They have a `method` field but no `id`:

```json
{
    "jsonrpc": "2.0",
    "method": "receive",
    "params": {
        "envelope": {
            "source": "+15559876543",
            "sourceDevice": 1,
            "timestamp": 1700000000000,
            "dataMessage": {
                "message": "Hey there!",
                "timestamp": 1700000000000
            }
        }
    }
}
```

## Methods used

### Outbound (siggy -> signal-cli)

| Method | Purpose |
|---|---|
| `send` | Send a message (also used for edits via `editTimestamp` param) |
| `listContacts` | Request the contact address book |
| `listGroups` | Request the list of groups |
| `sendSyncRequest` | Request a sync from the primary device |
| `sendReaction` | Send an emoji reaction to a message |
| `remoteDelete` | Delete a message for all recipients |
| `sendTypingIndicator` | Send typing started/stopped indicator |
| `sendReceipt` | Send a read receipt for one or more messages |
| `updateGroup` | Create/rename group, add/remove members |
| `quitGroup` | Leave a group |
| `block` | Block a contact or group |
| `unblock` | Unblock a contact or group |
| `setExpiration` | Set disappearing message timer |
| `updateProfile` | Update own Signal profile (name, about, emoji) |
| `listIdentities` | List known identity keys for contacts |
| `trust` | Trust a contact's identity key |
| `sendMessageRequestResponse` | Accept or delete a message request |

### Inbound notifications (signal-cli -> siggy)

| Method | Purpose | Maps to |
|---|---|---|
| `receive` | Incoming message | `SignalEvent::MessageReceived` |
| `receiveTyping` | Typing indicator | `SignalEvent::TypingIndicator` |
| `receiveReceipt` | Delivery/read receipt | `SignalEvent::ReceiptReceived` |

Incoming `receive` envelopes may also contain:

| Envelope field | Purpose | Maps to |
|---|---|---|
| `dataMessage.reaction` | Incoming reaction | `SignalEvent::ReactionReceived` |
| `dataMessage.remoteDelete` | Remote delete request | `SignalEvent::RemoteDeleteReceived` |
| `dataMessage.quote` | Quoted reply metadata | `quote` field on `SignalMessage` |
| `editMessage` | Edited message | `SignalEvent::EditReceived` |
| `syncMessage.sentMessage` | Outgoing sync (own messages from other devices) | Same as above, with `is_outgoing = true` |
| `syncMessage.readMessages` | Read sync from other devices | `SignalEvent::ReadSyncReceived` |
| `dataMessage.sticker` | Sticker message | Body set to `[Sticker: emoji]` |
| `dataMessage.textStyles` / `bodyRanges` | Text formatting (bold, italic, etc.) | `text_styles` field on `SignalMessage` |
| `dataMessage.expiresInSeconds` | Disappearing message timer | `expires_in_seconds` on `SignalMessage` |
| `dataMessage.isViewOnce` | View-once message flag | Body set to `[View-once message]` |
| `callMessage` | Missed call notification | `SignalEvent::SystemMessage` |

## Parsing logic

The stdout reader in `SignalClient` determines the message type by checking
which fields are present:

1. If `method` is present -> it's a notification, parse based on method name
2. If `id` and `result`/`error` are present -> it's a response, look up the
   method via `pending_requests[id]` and parse accordingly
3. Unknown methods are logged and discarded

## Sync messages

Messages sent from the primary device arrive as sync messages. They are
identified by having `is_outgoing = true` in the parsed `SignalMessage`.
The `destination` field indicates the recipient, and the message is routed
to the appropriate conversation.
