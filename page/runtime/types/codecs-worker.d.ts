/** Serve the `codecs` service in this Worker (`scope`, its global by default). Returns a function that stops listening. */
export declare function serveCodecs(options?: { scope?: { addEventListener(type: 'message', fn: (e: MessageEvent) => void): void; removeEventListener(type: 'message', fn: (e: MessageEvent) => void): void } }): () => void;
