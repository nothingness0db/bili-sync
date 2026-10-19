import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import test from 'node:test';
import { runInNewContext } from 'node:vm';
import ts from 'typescript';

// Run the actual page handlers with controlled API completion order.
const source = readFileSync(
	new URL('../src/routes/video/[id]/+page.svelte', import.meta.url),
	'utf8'
).match(/<script lang="ts">([\s\S]*?)<\/script>/)[1];
const ast = ts.createSourceFile('page.ts', source, ts.ScriptTarget.Latest, true, ts.ScriptKind.TS);
const handlers = [
	'loadVideoDetail',
	'handleReset',
	'handleClearAndReset',
	'handleStatusEditorSubmit'
];
const statements = ast.statements.filter(
	(node) =>
		ts.isVariableStatement(node) ||
		(ts.isFunctionDeclaration(node) && handlers.includes(node.name?.text))
);
const { outputText } = ts.transpileModule(
	statements.map((node) => node.getText(ast)).join('\n') +
		`;globalThis.pageTest = {${handlers.join(',')}, getData: () => videoData,
		getLoading: () => statusEditorLoading, destroy: () => { videoSequence += 1; }};`,
	{ compilerOptions: { target: ts.ScriptTarget.ES2022 } }
);

function deferred() {
	let resolve;
	let reject;
	const promise = new Promise((yes, no) => {
		resolve = yes;
		reject = no;
	});
	return { promise, resolve, reject };
}

function harness() {
	const pending = [];
	const notifications = [];
	const context = {
		$page: { params: { id: '1' } },
		console: { error() {} },
		toast: Object.fromEntries(
			['success', 'error', 'warning', 'info'].map((kind) => [
				kind,
				(message) => notifications.push({ kind, message })
			])
		),
		api: {
			getVideo: async (id) => ({ data: { video: { id, name: `video-${id}` }, pages: [] } }),
			...Object.fromEntries(
				['resetVideoStatus', 'clearAndResetVideoStatus', 'updateVideoStatus'].map((method) => [
					method,
					(id) => {
						const request = deferred();
						pending.push({ ...request, id });
						return request.promise;
					}
				])
			)
		}
	};
	runInNewContext(outputText, context);
	return { context, page: context.pageTest, pending, notifications };
}

for (const handler of handlers.slice(1)) {
	for (const outcome of ['success', 'error']) {
		test(`${handler} ignores stale ${outcome} after switching videos`, async () => {
			const h = harness();
			await h.page.loadVideoDetail();
			const operation = h.page[handler]({});
			assert.equal(h.pending[0].id, 1);
			h.context.$page.params.id = '2';
			await h.page.loadVideoDetail();
			if (outcome === 'success') {
				h.pending[0].resolve({
					data: { resetted: true, success: true, video: { id: 1 }, pages: [] }
				});
			} else h.pending[0].reject({ message: 'old request failed' });
			await operation;
			assert.equal(h.page.getData().video.id, 2);
			assert.equal(h.page.getLoading(), false);
			assert.equal(h.notifications.length, 0);
		});
	}
}

test('a late edit cannot replace a newer mutation on the same video', async () => {
	const h = harness();
	await h.page.loadVideoDetail();
	const edit = h.page.handleStatusEditorSubmit({});
	const clear = h.page.handleClearAndReset();
	h.pending[1].resolve({ data: { video: { id: 1, name: 'cleared' } } });
	await clear;
	h.pending[0].resolve({ data: { success: true, video: { id: 1, name: 'old edit' }, pages: [] } });
	await edit;
	assert.equal(h.page.getData().video.name, 'cleared');
	assert.equal(h.page.getLoading(), false);
	assert.equal(h.notifications.length, 1);
});

test('unmounted detail page ignores a pending modification', async () => {
	const h = harness();
	await h.page.loadVideoDetail();
	const operation = h.page.handleReset(false);
	h.page.destroy();
	h.pending[0].resolve({
		data: { resetted: true, video: { id: 1, name: 'late result' }, pages: [] }
	});
	await operation;
	assert.equal(h.page.getData().video.name, 'video-1');
	assert.equal(h.notifications.length, 0);
});
