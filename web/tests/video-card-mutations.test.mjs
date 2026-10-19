import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import test from 'node:test';
import { runInNewContext } from 'node:vm';
import ts from 'typescript';

const source = readFileSync(
	new URL('../src/lib/components/video-card.svelte', import.meta.url),
	'utf8'
)
	.match(/<script lang="ts">([\s\S]*?)<\/script>/)[1]
	.replace(/\bexport let\b/g, 'let');
const ast = ts.createSourceFile('card.ts', source, ts.ScriptTarget.Latest, true, ts.ScriptKind.TS);
const handlers = ['handleReset', 'handleClearAndReset'];
const statements = ast.statements.filter(
	(node) =>
		ts.isVariableStatement(node) ||
		(ts.isFunctionDeclaration(node) && handlers.includes(node.name?.text)) ||
		(ts.isExpressionStatement(node) && node.expression.expression?.getText(ast) === 'onDestroy')
);
const { outputText } = ts.transpileModule(
	statements.map((node) => node.getText(ast)).join('\n') +
		`;globalThis.cardTest = {${handlers.join(',')},
		init: (callback) => { video = {id: 1}; onReset = callback; onClearAndReset = callback; },
		setPending: () => { resetting = true; resetDialogOpen = true; clearAndResetting = true; clearAndResetDialogOpen = true; },
		changeVideo: () => { video = {id: 2}; },
		getState: () => ({resetting, resetDialogOpen, clearAndResetting, clearAndResetDialogOpen})};`,
	{ compilerOptions: { target: ts.ScriptTarget.ES2022 } }
);

function harness() {
	let resolve;
	const pending = new Promise((done) => (resolve = done));
	let destroy;
	const context = { onDestroy: (callback) => (destroy = callback) };
	runInNewContext(outputText, context);
	context.cardTest.init(() => pending);
	return { card: context.cardTest, resolve, destroy: () => destroy() };
}

for (const handler of handlers) {
	for (const change of ['destroy', 'changeVideo']) {
		test(`${handler} does not clear another video's bound dialog or loading after ${change}`, async () => {
			const h = harness();
			const operation = h.card[handler]();
			if (change === 'destroy') h.destroy();
			else h.card.changeVideo();
			// 模拟新页面已经打开自己的弹窗并开始操作，绑定值属于新页面。
			h.card.setPending();
			h.resolve();
			await operation;
			for (const value of Object.values(h.card.getState())) assert.equal(value, true);
		});
	}
	test(`${handler} clears its own loading and dialog when the current operation finishes`, async () => {
		const h = harness();
		h.card.setPending();
		const operation = h.card[handler]();
		h.resolve();
		await operation;
		const state = h.card.getState();
		if (handler === 'handleReset') {
			assert.equal(state.resetting, false);
			assert.equal(state.resetDialogOpen, false);
		} else {
			assert.equal(state.clearAndResetting, false);
			assert.equal(state.clearAndResetDialogOpen, false);
		}
	});
}
