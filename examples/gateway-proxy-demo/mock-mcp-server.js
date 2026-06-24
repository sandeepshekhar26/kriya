#!/usr/bin/env node
//
// mock-mcp-server.js — a tiny, zero-dependency MCP server over stdio.
//
// This stands in for "an app we didn't write" that already ships its own MCP
// server. It has NO governance of its own: any client that can speak to it can
// call delete_note and the note is gone. That is exactly the point of this demo
// — kriya-gateway sits in front of THIS process and adds policy + human approval
// + a signed audit trail, with zero changes to a single line below.
//
// Protocol: newline-delimited JSON-RPC 2.0 over stdin/stdout.
//   - stdout carries ONLY JSON-RPC messages (one object per line).
//   - all logging goes to stderr (never stdout — that would corrupt the stream).
//
// Run it directly to sanity-check it:
//   printf '%s\n' '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}' | node mock-mcp-server.js

"use strict";

const PROTOCOL_VERSION = "2025-06-18";

// In-memory "database" — seeded so a read returns something real, and a delete
// has something to destroy. Nothing here is persisted; restart = fresh seed.
let nextId = 4;
const notes = [
  { id: "note-1", title: "Groceries", body: "Oat milk, spinach, coffee." },
  { id: "note-2", title: "Standup", body: "Ship the gateway demo; unblock review." },
  { id: "note-3", title: "Idea", body: "Govern any MCP server with zero changes." },
];

// --- tool catalogue --------------------------------------------------------
// Deliberately spans the three governance classes the gateway cares about:
//   read-like (list_notes, get_note), benign write (create_note),
//   and DESTRUCTIVE (delete_note). The mock treats them all identically;
//   the gateway is what tells them apart.
const TOOLS = [
  {
    name: "list_notes",
    description: "List all notes (id and title). Read-only.",
    inputSchema: { type: "object", properties: {}, additionalProperties: false },
  },
  {
    name: "get_note",
    description: "Get a single note's full contents by id. Read-only.",
    inputSchema: {
      type: "object",
      properties: { id: { type: "string", description: "The note id." } },
      required: ["id"],
      additionalProperties: false,
    },
  },
  {
    name: "create_note",
    description: "Create a new note with a title and body.",
    inputSchema: {
      type: "object",
      properties: {
        title: { type: "string", description: "Note title." },
        body: { type: "string", description: "Note body." },
      },
      required: ["title", "body"],
      additionalProperties: false,
    },
  },
  {
    name: "delete_note",
    description: "Permanently delete a note by id. Destructive — cannot be undone.",
    inputSchema: {
      type: "object",
      properties: { id: { type: "string", description: "The note id to delete." } },
      required: ["id"],
      additionalProperties: false,
    },
  },
];

// --- tool implementations --------------------------------------------------
// Each returns an MCP CallToolResult: { content: [{type:"text", text}], isError }.
function callTool(name, args) {
  const a = args || {};
  switch (name) {
    case "list_notes": {
      const lines = notes.map((n) => `${n.id}: ${n.title}`).join("\n");
      return text(notes.length ? lines : "(no notes)");
    }
    case "get_note": {
      const n = notes.find((x) => x.id === a.id);
      if (!n) return errorText(`No note with id "${a.id}".`);
      return text(`${n.title}\n\n${n.body}`);
    }
    case "create_note": {
      if (typeof a.title !== "string" || typeof a.body !== "string") {
        return errorText("create_note requires string 'title' and 'body'.");
      }
      const note = { id: `note-${nextId++}`, title: a.title, body: a.body };
      notes.push(note);
      return text(`Created ${note.id}: ${note.title}`);
    }
    case "delete_note": {
      const i = notes.findIndex((x) => x.id === a.id);
      if (i === -1) return errorText(`No note with id "${a.id}".`);
      const [removed] = notes.splice(i, 1);
      return text(`Deleted ${removed.id}: ${removed.title}`);
    }
    default:
      return errorText(`Unknown tool: ${name}`);
  }
}

const text = (s) => ({ content: [{ type: "text", text: s }], isError: false });
const errorText = (s) => ({ content: [{ type: "text", text: s }], isError: true });

// --- JSON-RPC plumbing -----------------------------------------------------
function send(obj) {
  process.stdout.write(JSON.stringify(obj) + "\n");
}
function reply(id, result) {
  send({ jsonrpc: "2.0", id, result });
}
function replyError(id, code, message) {
  send({ jsonrpc: "2.0", id, error: { code, message } });
}

function handle(msg) {
  const { id, method, params } = msg;
  // A notification has no id and expects no response.
  const isNotification = id === undefined || id === null;

  switch (method) {
    case "initialize":
      reply(id, {
        protocolVersion: PROTOCOL_VERSION,
        capabilities: { tools: {} },
        serverInfo: { name: "mock-notes", version: "0.1.0" },
      });
      return;

    case "notifications/initialized":
      // No-op: it's a notification, so we send nothing back.
      return;

    case "ping":
      reply(id, {});
      return;

    case "tools/list":
      reply(id, { tools: TOOLS });
      return;

    case "tools/call": {
      const name = params && params.name;
      const args = params && params.arguments;
      process.stderr.write(`[mock-notes] tools/call ${name}\n`);
      reply(id, callTool(name, args));
      return;
    }

    default:
      if (isNotification) return; // ignore unknown notifications
      replyError(id, -32601, `Method not found: ${method}`);
  }
}

// Read newline-delimited JSON from stdin; tolerate partial chunks.
let buffer = "";
process.stdin.setEncoding("utf8");
process.stdin.on("data", (chunk) => {
  buffer += chunk;
  let nl;
  while ((nl = buffer.indexOf("\n")) !== -1) {
    const line = buffer.slice(0, nl).trim();
    buffer = buffer.slice(nl + 1);
    if (!line) continue;
    let msg;
    try {
      msg = JSON.parse(line);
    } catch (e) {
      process.stderr.write(`[mock-notes] bad JSON: ${line}\n`);
      continue;
    }
    handle(msg);
  }
});
process.stdin.on("end", () => process.exit(0));

process.stderr.write("[mock-notes] ready (stdio JSON-RPC; an app with NO governance of its own)\n");
