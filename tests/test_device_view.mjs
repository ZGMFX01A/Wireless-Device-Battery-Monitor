import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { beforeEach, test } from 'node:test';

// Minimal DOM surface for exercising the real renderer and its retained listeners.
class Element {
    constructor(tag, type = 1, data = '') {
        Object.assign(this, { nodeName: tag.toUpperCase(), nodeType: type, data,
            childNodes: [], parentNode: null, attrs: new Map(), style: {}, dataset: {}, events: {} });
    }
    get attributes() { return [...this.attrs].map(([name, value]) => ({ name, value })); }
    setAttribute(key, value) { this.attrs.set(key, String(value)); }
    getAttribute(key) { return this.attrs.get(key) ?? null; }
    hasAttribute(key) { return this.attrs.has(key); }
    removeAttribute(key) { this.attrs.delete(key); }
    get className() { return this.getAttribute('class') || ''; }
    set className(value) { this.setAttribute('class', value); }
    get classList() {
        return {
            contains: value => this.className.split(' ').includes(value),
            add: value => { this.className += ` ${value}`; },
            remove: value => { this.className = this.className.split(' ').filter(x => x !== value).join(' '); },
        };
    }
    get children() { return this.childNodes.filter(x => x.nodeType === 1); }
    get lastChild() { return this.childNodes.at(-1); }
    get textContent() { return this.nodeType === 3 ? this.data : this.childNodes.map(x => x.textContent).join(''); }
    set textContent(value) {
        while (this.lastChild) this.lastChild.remove();
        this.append(new Element('#text', 3, String(value)));
    }
    append(...nodes) { for (const node of nodes) { node.remove(); node.parentNode = this; this.childNodes.push(node); } }
    remove() {
        if (this.parentNode) this.parentNode.childNodes.splice(this.parentNode.childNodes.indexOf(this), 1);
        this.parentNode = null;
    }
    replaceWith(node) {
        const parent = this.parentNode;
        if (!parent) return;
        const index = parent.childNodes.indexOf(this);
        node.remove(); parent.childNodes[index] = node; node.parentNode = parent; this.parentNode = null;
    }
    insertBefore(node, reference) {
        if (node === reference) return;
        node.remove();
        this.childNodes.splice(reference ? this.childNodes.indexOf(reference) : this.childNodes.length, 0, node);
        node.parentNode = this;
    }
    addEventListener(type, callback) { (this.events[type] ??= []).push(callback); }
    dispatch(type, options = {}) {
        for (const callback of this.events[type] || []) callback({ stopPropagation() {}, preventDefault() {}, ...options });
    }
    focus() {}
    select() {}
}

const urlFor = source => `data:text/javascript;base64,${Buffer.from(source).toString('base64')}`;
const domUrl = urlFor(readFileSync(new URL('../web_ui/dom.js', import.meta.url), 'utf8'));
const viewSource = readFileSync(new URL('../web_ui/device_view.js', import.meta.url), 'utf8');
const { createDeviceRenderer } = await import(urlFor(viewSource.replace("'./dom.js'", JSON.stringify(domUrl))));
const find = (node, className) => node.classList.contains(className) ? node
    : node.childNodes.map(child => find(child, className)).find(Boolean);
const t = key => key;
let elements;
beforeEach(() => {
    elements = Object.fromEntries(['deviceList', 'emptyState', 'deviceStatus'].map(id => [id, new Element('div')]));
    globalThis.document = { createElement: tag => new Element(tag), createElementNS: (_, tag) => new Element(tag),
        getElementById: id => elements[id] };
    globalThis.Node = { TEXT_NODE: 3 };
});

function stateFor(mice) {
    return { mice, keyboard: null, bluetooth_devices: [], config: { effective_ui_language: 'en-US' }, read_state: 'ok' };
}
const mouse = (id, name) => ({ device_id: id, product_id: 0xc539, name, brand: 'logitech',
    online: true, percentage: 70, last_update: 1 });

test('reopening a renamed card edits the current name and can restore the original', () => {
    const saves = [];
    const render = createDeviceRenderer({ onRename: (...args) => saves.push(args) });
    const state = stateFor([mouse('a', 'Original')]);
    render(state, t);
    const card = elements.deviceList.children[0];
    find(card, 'card-btn-edit').dispatch('click');
    find(card, 'device-name-input').value = 'Office';
    find(card, 'device-name-input').dispatch('keydown', { key: 'Enter' });
    state.mice[0] = { ...state.mice[0], name: 'Office' };
    render(state, t);
    find(card, 'card-btn-edit').dispatch('click');
    const input = find(card, 'device-name-input');
    assert.equal(input.value, 'Office');
    input.value = 'Original'; input.dispatch('keydown', { key: 'Enter' });
    assert.deepEqual(saves, [['a', 'Office'], ['a', 'Original']]);
});

test('same-model receivers retain their identity and editor when reordered', () => {
    const saves = [];
    const render = createDeviceRenderer({ onRename: (...args) => saves.push(args) });
    render(stateFor([mouse('a', 'A'), mouse('b', 'B')]), t);
    const firstCard = elements.deviceList.children[0];
    render(stateFor([mouse('b', 'B'), mouse('a', 'A')]), t);
    assert.equal(elements.deviceList.children[1], firstCard);
    const card = elements.deviceList.children[0];
    find(card, 'card-btn-edit').dispatch('click');
    const input = find(card, 'device-name-input');
    assert.equal(input.value, 'B');
    input.value = 'B renamed'; input.dispatch('blur');
    assert.deepEqual(saves, [['b', 'B renamed']]);
});

test('polling preserves input and Escape cancels without saving', () => {
    const saves = [];
    const render = createDeviceRenderer({ onRename: (...args) => saves.push(args) });
    const state = stateFor([mouse('a', 'Original')]);
    render(state, t);
    const card = elements.deviceList.children[0];
    find(card, 'card-btn-edit').dispatch('click');
    const input = find(card, 'device-name-input'); input.value = 'Draft';
    state.mice[0].percentage = 69; render(state, t);
    assert.equal(find(card, 'device-name-input'), input);
    assert.equal(input.value, 'Draft');
    input.dispatch('keydown', { key: 'Escape' }); input.dispatch('blur');
    assert.equal(find(card, 'device-name').textContent, 'Original');
    assert.deepEqual(saves, []);
});
