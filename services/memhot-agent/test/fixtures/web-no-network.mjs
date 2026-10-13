import dns from 'node:dns/promises';
import { syncBuiltinESMExports } from 'node:module';

// Only plugin DNS preflight is replaced. Every HTTP body still crosses the real worker RPC.
dns.lookup = async () => [{ address: '93.184.216.34', family: 4 }];
syncBuiltinESMExports();
