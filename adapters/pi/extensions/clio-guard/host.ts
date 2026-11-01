// The client of the tool host (crates/clio-toolhost): one connection per call, one JSON line each
// way, a time limit, and a failure that always means "the tool failed".

import { createConnection } from "node:net";

export interface HostResponse {
	id: string;
	ok: boolean;
	content: string;
	decision: { outcome: string; rule: string; reason: string };
	tainted: boolean;
	external: boolean;
}

const MAX_RESPONSE = 8 * 1024 * 1024;

export function callHost(
	socket: string,
	request: Record<string, unknown>,
	timeoutMs: number,
	signal: AbortSignal | undefined,
): Promise<HostResponse> {
	return new Promise((resolve, reject) => {
		let sent = false;
		let done = false;
		const chunks: Buffer[] = [];
		let size = 0;
		const conn = createConnection(socket);
		const finish = (err: Error | undefined, value?: HostResponse) => {
			if (done) return;
			done = true;
			clearTimeout(timer);
			signal?.removeEventListener("abort", onAbort);
			conn.destroy();
			if (err) reject(err);
			else resolve(value as HostResponse);
		};
		const unknown = (why: string) =>
			new Error(`the tool host ${why}; the call may or may not have run, check before repeating it`);
		const timer = setTimeout(() => finish(unknown(`did not answer in ${timeoutMs} ms`)), timeoutMs);
		const onAbort = () => finish(new Error("the call was aborted"));
		signal?.addEventListener("abort", onAbort, { once: true });
		conn.on("connect", () => {
			sent = true;
			conn.write(JSON.stringify(request) + "\n");
		});
		conn.on("data", (d: Buffer) => {
			chunks.push(d);
			size += d.length;
			if (size > MAX_RESPONSE) return finish(unknown("sent an answer that is too large"));
			if (d.includes(10)) {
				try {
					const line = Buffer.concat(chunks).toString("utf8").split("\n")[0];
					const r = JSON.parse(line) as HostResponse;
					if (typeof r.ok !== "boolean" || typeof r.content !== "string" || r.id !== request.id) {
						return finish(unknown("sent an answer that does not match the request"));
					}
					finish(undefined, r);
				} catch {
					finish(unknown("sent an answer that is not valid JSON"));
				}
			}
		});
		conn.on("error", (e: Error) =>
			finish(
				sent
					? unknown(`failed (${e.message})`)
					: new Error(`the tool host is not available (${e.message}): nothing was run`),
			),
		);
		conn.on("close", () =>
			finish(
				sent ? unknown("closed the connection without answering") : new Error("the tool host is not available: nothing was run"),
			),
		);
	});
}
