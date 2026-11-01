// The versioned system prompt of Clio (prompts/system/clio-agent@N.md, S-09). It is read once,
// when the extension loads, and any problem is a thrown error: an extension that does not load
// leaves Pi with no tools at all (they are all registered here), so the failure is closed.

import { readFileSync } from "node:fs";
import { basename } from "node:path";

const NAME = /^([a-z0-9-]+)@([0-9]+)\.md$/;

export interface ClioPrompt {
	/** `clio-agent@4`. */
	version: string;
	text: string;
}

export function loadPrompt(env: NodeJS.ProcessEnv): ClioPrompt {
	const file = env.CLIO_GUARD_PROMPT_FILE;
	const version = env.CLIO_GUARD_PROMPT_VERSION;
	if (!file || !version) {
		throw new Error("clio-guard: CLIO_GUARD_PROMPT_FILE and CLIO_GUARD_PROMPT_VERSION must be set; refusing to run without the Clio prompt");
	}
	const m = NAME.exec(basename(file));
	if (!m || `${m[1]}@${m[2]}` !== version) {
		throw new Error(`clio-guard: the prompt file ${basename(file)} does not match the version ${version}`);
	}
	let raw: string;
	try {
		raw = readFileSync(file, "utf8");
	} catch (e) {
		throw new Error(`clio-guard: cannot read the prompt file ${file}: ${(e as Error).message}`);
	}
	const nl = raw.indexOf("\n");
	if (raw.slice(0, nl < 0 ? raw.length : nl).trim() !== `version: ${m[2]}`) {
		throw new Error(`clio-guard: the first line of ${basename(file)} must be "version: ${m[2]}"`);
	}
	const text = nl < 0 ? "" : raw.slice(nl + 1).trim();
	if (!text) throw new Error(`clio-guard: the prompt file ${basename(file)} is empty`);
	return { version, text };
}
