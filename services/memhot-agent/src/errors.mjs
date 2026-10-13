export class HarnessError extends Error {
  constructor(code, message) {
    super(message);
    this.name = 'HarnessError';
    this.code = code;
  }
}

export function failure(error) {
  return error instanceof HarnessError
    ? { type: 'failed', code: error.code, message: error.message }
    : { type: 'failed', code: 'AGENT_FAILED', message: 'The agent run could not finish.' };
}
