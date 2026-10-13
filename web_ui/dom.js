// DOM 查询失败必须明确暴露，避免页面只显示一部分却被认为已就绪。
export function el(id) {
    const node = document.getElementById(id);
    if (!node) throw new Error(`Missing GUI element: ${id}`);
    return node;
}

// 外部名称、ID 和状态只写入文本节点或属性，不能进入 HTML 模板。
export function node(tag, options = {}) {
    const result = document.createElement(tag);
    if (options.className) result.className = options.className;
    if (options.text !== undefined) result.textContent = String(options.text);
    return result;
}

// 复用原模态框的旋转加载指示与居中文字，内容仍使用安全文本节点。
export function loadingNode(text) {
    const loading = node('div', { className: 'modal-loading' });
    loading.append(node('div', { className: 'spinner' }), node('div', { text }));
    return loading;
}

// 每个页面实例管理自己的提示计时器，不共享跨页面状态。
export function createToast() {
    let timer;
    return function toast(text) {
        const box = el('toastNotification');
        clearTimeout(timer);
        box.textContent = String(text);
        box.style.display = 'block';
        timer = setTimeout(() => { box.style.display = 'none'; }, 2200);
    };
}

// 调用 Python 接口的目的与约束集中在边界：无桥接或接口异常都抛出。
export function api(method, ...args) {
    if (!window.pywebview?.api?.[method]) throw new Error(`Python API unavailable: ${method}`);
    return window.pywebview.api[method](...args);
}

// 同步已有节点的实际文本、属性和子结构，保留卡片及进度条节点以延续动画。
export function syncNode(target, source) {
    if (target.nodeType === Node.TEXT_NODE) {
        if (target.data !== source.data) target.data = source.data;
        return;
    }
    syncAttributes(target, source);
    const children = [...source.childNodes];
    children.forEach((fresh, index) => {
        const current = target.childNodes[index];
        if (!current) target.append(fresh);
        else if (current.nodeType !== fresh.nodeType || current.nodeName !== fresh.nodeName) current.replaceWith(fresh);
        else syncNode(current, fresh);
    });
    while (target.childNodes.length > children.length) target.lastChild.remove();
}

// 属性仍来自安全 DOM 构造器；只写变化值，避免重复触发样式计算。
function syncAttributes(target, source) {
    [...target.attributes].forEach(attribute => {
        if (!source.hasAttribute(attribute.name)) target.removeAttribute(attribute.name);
    });
    [...source.attributes].forEach(attribute => {
        if (target.getAttribute(attribute.name) !== attribute.value) target.setAttribute(attribute.name, attribute.value);
    });
}
