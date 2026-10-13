import Ajv from 'ajv';
import { HarnessError } from './errors.mjs';

export const LIMITS = Object.freeze({
  inputFrame: 8 * 1024 * 1024,
  modelSse: 4 * 1024 * 1024,
  outputFrame: 1024 * 1024,
  totalOutput: 16 * 1024 * 1024 - 1024,
  context: 512 * 1024,
  maxToolCalls: 32,
  toolArgs: 32 * 1024,
  toolSchema: 32 * 1024,
  maxTools: 16,
  maxSteps: 32,
  rpcTimeoutMs: 120_000,
  runTimeoutMs: 15 * 60_000,
});

const ajv = new Ajv({ strict: true, allErrors: false, validateFormats: false });
const toolName = /^[A-Za-z][A-Za-z0-9_]{0,63}$/;
export const isObject = value => value !== null && typeof value === 'object' && !Array.isArray(value);
const bytes = value => Buffer.byteLength(JSON.stringify(value));
const invalid = () => { throw new HarnessError('INVALID_START', 'The agent input is invalid.'); };
const string = (value, max, empty = false) => typeof value === 'string' &&
  value.length <= max && (empty || value.trim().length > 0);
const integer = (value, max) => Number.isSafeInteger(value) && value > 0 && value <= max;

function boundedSchema(schema) {
  if (!isObject(schema) || schema.type !== 'object' || bytes(schema) > LIMITS.toolSchema) invalid();
  let nodes = 0;
  const walk = (value, depth) => {
    if (++nodes > 1024 || depth > 12) invalid();
    if (Array.isArray(value)) return value.forEach(item => walk(item, depth + 1));
    if (!isObject(value)) return;
    for (const [key, child] of Object.entries(value)) {
      // No remote/local references, async validators, or user-supplied regex evaluation.
      if (['$ref', '$dynamicRef', '$recursiveRef', '$async', '$id', 'pattern', 'patternProperties'].includes(key)) invalid();
      walk(child, depth + 1);
    }
  };
  walk(schema, 0);
  try { return ajv.compile(schema); } catch { invalid(); }
}

function checkMessage(message, names) {
  if (!isObject(message) || !['system', 'user', 'assistant', 'toolResult'].includes(message.role)) invalid();
  if (!Number.isFinite(message.timestamp) || message.timestamp < 0) invalid();
  if (message.role === 'assistant') {
    if (!string(message.api, 100) || message.provider !== 'memhot' || !string(message.model, 200) ||
      !isObject(message.usage) || !['stop', 'length', 'toolUse', 'error', 'aborted'].includes(message.stopReason)) invalid();
  }
  if (message.role === 'toolResult' && (!string(message.toolCallId, 200) ||
    !toolName.test(message.toolName) || typeof message.isError !== 'boolean')) invalid();
  if (message.role === 'system') {
    if (message.toolsAdded !== undefined) {
      if (!Array.isArray(message.toolsAdded) || message.toolsAdded.length > LIMITS.maxTools) invalid();
      for (const definition of message.toolsAdded) {
        if (!names.has(definition.name)) invalid();
        boundedSchema(definition.parameters);
      }
    }
    if (message.toolsRemoved !== undefined && (!Array.isArray(message.toolsRemoved) || message.toolsRemoved.some(tool => !names.has(tool.name)))) invalid();
    if (message.sections !== undefined && (!isObject(message.sections) || Object.values(message.sections).some(value => value !== null && typeof value !== 'string'))) invalid();
  }
  if (typeof message.content === 'string') {
    if (!['system', 'user'].includes(message.role) || !string(message.content, 256_000, true)) invalid();
    return;
  }
  if (!Array.isArray(message.content) || message.content.length > 64) invalid();
  for (const part of message.content) {
    if (!isObject(part)) invalid();
    if (part.type === 'text') {
      if (!string(part.text, 256_000, true) || (part.textSignature !== undefined && !string(part.textSignature, 256_000, true))) invalid();
    } else if (part.type === 'thinking' && message.role === 'assistant') {
      if (!string(part.thinking, 256_000, true) || (part.thinkingSignature !== undefined && !string(part.thinkingSignature, 256_000, true))) invalid();
    } else if (part.type === 'toolCall' && message.role === 'assistant') {
      if (!string(part.id, 200) || !toolName.test(part.name) || !isObject(part.arguments) ||
        bytes(part.arguments) > LIMITS.toolArgs || (part.thoughtSignature !== undefined && !string(part.thoughtSignature, 256_000, true))) invalid();
    } else invalid();
  }
}

export function validateStart(value) {
  if (!isObject(value) || value.type !== 'start' || !string(value.runId, 128) ||
    !['digest', 'research', 'discussion', 'translation', 'review'].includes(value.mode) ||
    !isObject(value.model) || !['openai', 'anthropic', 'gemini'].includes(value.model.protocol) ||
    !string(value.model.model, 200) || /[\s\x00-\x1f]/.test(value.model.model) ||
    !string(value.prompt, 256_000, true) || !string(value.instructions, 64_000, true) ||
    !integer(value.maxSteps, LIMITS.maxSteps) || !integer(value.maxOutputTokens, 32_768) ||
    !Array.isArray(value.tools) || value.tools.length > LIMITS.maxTools) invalid();
  if (Object.keys(value).some(key => !['type', 'runId', 'mode', 'model', 'prompt', 'instructions', 'tools', 'maxSteps', 'maxOutputTokens', 'messagesJson'].includes(key)) ||
    Object.keys(value.model).some(key => !['protocol', 'model'].includes(key))) invalid();
  const names = new Set();
  const tools = value.tools.map(item => {
    if (!isObject(item) || !toolName.test(item.name) || !string(item.description, 4096) || names.has(item.name) ||
      Object.keys(item).some(key => !['name', 'description', 'inputSchema'].includes(key))) invalid();
    names.add(item.name);
    return { ...item, validate: boundedSchema(item.inputSchema) };
  });
  let messages = [];
  if (value.messagesJson !== undefined) {
    if (typeof value.messagesJson !== 'string' || Buffer.byteLength(value.messagesJson) > LIMITS.context) invalid();
    try { messages = JSON.parse(value.messagesJson); } catch { invalid(); }
  }
  if (!Array.isArray(messages) || messages.length > 256 || bytes(messages) > LIMITS.context) invalid();
  messages.forEach(message => checkMessage(message, names));
  if (!value.prompt.trim() && messages.length === 0) invalid();
  return { ...value, tools, messages };
}

export function validToolArgs(args, validate) {
  return isObject(args) && bytes(args) <= LIMITS.toolArgs && validate(args);
}
