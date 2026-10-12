import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import test from 'node:test';
import { runInNewContext } from 'node:vm';
import ts from 'typescript';

// Exercise the production manager with controlled socket events and timers.
const source = readFileSync(new URL('../src/lib/ws.ts', import.meta.url), 'utf8').replace(
	/^import .*;\n/gm,
	''
);
const { outputText } = ts.transpileModule(source, {
	compilerOptions: { module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2022 }
});

function harness({ constructorFailures = 0 } = {}) {
	const sockets = [];
	const timers = new Map();
	let timerId = 0;
	class Socket {
		constructor() {
			if (constructorFailures-- > 0) throw new Error('Cannot create socket');
			this.sent = [];
			sockets.push(this);
		}
		send(data) {
			this.sent.push(JSON.parse(data));
		}
		close() {
			this.closeRequested = true;
		}
		open() {
			this.onopen();
		}
		finishClose() {
			this.onclose();
		}
		fail() {
			this.onerror(new Error('Network failure'));
			this.finishClose();
		}
	}
	const exports = {};
	runInNewContext(outputText, {
		exports,
		api: { getAuthToken: () => 'test-token' },
		toast: { error() {} },
		console: { log() {}, error() {} },
		window: { location: { protocol: 'http:', host: 'test' } },
		btoa: (value) => Buffer.from(value).toString('base64'),
		WebSocket: Socket,
		setTimeout: (fn) => {
			const id = ++timerId;
			timers.set(id, fn);
			return id;
		},
		clearTimeout: (id) => timers.delete(id)
	});
	return {
		manager: exports.wsManager,
		sockets,
		timers,
		retry() {
			assert.equal(timers.size, 1);
			const [id, fn] = timers.entries().next().value;
			timers.delete(id);
			fn();
		}
	};
}

const subscriptions = (socket) => socket.sent.map((message) => message.subscribe).filter(Boolean);

test('restores all active subscriptions after the first connection fails', async () => {
	const h = harness();
	const received = [];
	h.manager.subscribeToLogs((data) => received.push(data));
	h.manager.subscribeToTasks(() => {});
	h.manager.subscribeToSysInfo(() => {});
	assert.equal(h.sockets.length, 1);
	h.sockets[0].fail();
	h.retry();
	h.sockets[1].open();
	assert.deepEqual(subscriptions(h.sockets[1]), ['logs', 'tasks', 'sysInfo']);
	h.sockets[1].onmessage({ data: JSON.stringify({ logs: 'restored' }) });
	assert.deepEqual(received, ['restored']);
});

test('cancels subscriptions before connection opens without sending them later', () => {
	const h = harness();
	const unsubscribe = h.manager.subscribeToLogs(() => {});
	unsubscribe();
	h.sockets[0].open();
	assert.deepEqual(h.sockets[0].sent, []);
	h.sockets[0].finishClose();
	assert.equal(h.timers.size, 0);
});

test('only restores subscriptions whose listeners remain after a disconnect', () => {
	const h = harness();
	const unsubscribeLogs = h.manager.subscribeToLogs(() => {});
	h.manager.subscribeToTasks(() => {});
	h.sockets[0].open();
	h.sockets[0].finishClose();
	unsubscribeLogs();
	h.retry();
	h.sockets[1].open();
	assert.deepEqual(subscriptions(h.sockets[1]), ['tasks']);
});

test('shares one server subscription until the last listener unsubscribes', () => {
	const h = harness();
	const first = h.manager.subscribeToLogs(() => {});
	const second = h.manager.subscribeToLogs(() => {});
	h.sockets[0].open();
	first();
	assert.deepEqual(h.sockets[0].sent, [{ subscribe: 'logs' }]);
	second();
	assert.deepEqual(h.sockets[0].sent, [{ subscribe: 'logs' }, { unsubscribe: 'logs' }]);
});

test('intentional disconnect prevents asynchronous close from scheduling a reconnect', () => {
	const h = harness();
	let received = false;
	h.manager.subscribeToLogs(() => (received = true));
	h.sockets[0].open();
	h.manager.disconnect();
	assert.equal(h.sockets[0].closeRequested, true);
	h.sockets[0].finishClose();
	h.sockets[0].onmessage({ data: JSON.stringify({ logs: 'stale' }) });
	assert.equal(h.timers.size, 0);
	assert.equal(received, false);
});

test('disconnect settles a pending connection and old callbacks cannot break a new one', async () => {
	const h = harness();
	const unsubscribe = h.manager.subscribeToLogs(() => {});
	const pending = h.manager.connect();
	const rejected = assert.rejects(pending, /主动断开/);
	h.manager.disconnect();
	await rejected;
	unsubscribe();
	h.manager.subscribeToLogs(() => {});
	h.sockets[1].open();
	h.sockets[0].finishClose();
	h.sockets[0].open();
	await h.manager.connect();
	assert.equal(h.sockets.length, 2);
	assert.equal(h.timers.size, 0);
	assert.deepEqual(subscriptions(h.sockets[1]), ['logs']);
});

test('recovers from a synchronous socket constructor failure', () => {
	const h = harness({ constructorFailures: 1 });
	h.manager.subscribeToLogs(() => {});
	h.retry();
	h.sockets[0].open();
	assert.deepEqual(subscriptions(h.sockets[0]), ['logs']);
});

test('stops retrying after five attempts and cancels a scheduled retry on logout', () => {
	const h = harness();
	h.manager.subscribeToLogs(() => {});
	for (let attempt = 0; attempt < 5; attempt++) {
		h.sockets[attempt].fail();
		h.retry();
	}
	h.sockets[5].fail();
	assert.equal(h.timers.size, 0);
	const other = harness();
	other.manager.subscribeToLogs(() => {});
	other.sockets[0].fail();
	assert.equal(other.timers.size, 1);
	other.manager.disconnect();
	assert.equal(other.timers.size, 0);
});
