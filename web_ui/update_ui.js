import { api, el } from './dom.js';

// 下载速率和大小以 MiB 显示，Python 回调仍传原始字节数。
const BYTES_PER_MIB = 1024 * 1024;

export class UpdateUi {
    constructor(options) {
        this.t = options.t;
        this.toast = options.toast;
        this.info = null;
        this.stage = 'idle';
        el('btnCheckUpdate').addEventListener('click', () => this.check());
        el('updateBtnLater').addEventListener('click', () => this.close());
        el('updateBtnStart').addEventListener('click', () => this.start());
        el('updateBtnCancel').addEventListener('click', () => this.cancelOrClose());
    }

    // 检查失败和已是最新版分开处理，异常信息不转换成成功提示。
    async check() {
        el('btnCheckUpdate').disabled = true;
        try {
            const info = await api('check_update');
            if (info.has_update) this.open(info);
            else this.toast(this.t('update.latest', { version: info.version }));
        } catch (error) {
            this.toast(this.t('update.check_failed', { msg: error.message || error }));
        } finally {
            el('btnCheckUpdate').disabled = false;
        }
    }

    // 发布说明必须使用 textContent，远端 Release 正文不能执行 HTML。
    open(info) {
        this.info = { ...info };
        this.stage = 'notes';
        el('updateVersionBadge').textContent = info.version;
        el('updateSizeTag').textContent = `${(info.size / BYTES_PER_MIB).toFixed(1)} MB`;
        el('updateNotesBox').textContent = info.notes;
        el('updateNotesView').style.display = 'flex';
        el('updateProgressView').style.display = 'none';
        el('updateModalOverlay').style.display = 'flex';
        el('updateBtnStart').disabled = false;
    }

    // 开始一次已检查的更新；接口直接拒绝被网页改写的资源。
    async start() {
        if (!this.info || this.stage !== 'notes') return;
        this.stage = 'starting';
        el('updateBtnStart').disabled = true;
        el('updateNotesView').style.display = 'none';
        el('updateProgressView').style.display = 'flex';
        el('updateProgressBar').style.width = '0%';
        el('updateSizeStat').textContent = `0.0 / ${(this.info.size / BYTES_PER_MIB).toFixed(1)} MB`;
        el('updateSpeedStat').textContent = '0.00 MB/s';
        el('updateBtnCancel').disabled = false;
        el('updateBtnCancel').textContent = this.t('update.cancel');
        try {
            await api('start_update_download', { url: this.info.url, size: this.info.size,
                digest: this.info.digest, version: this.info.version });
        } catch (error) {
            this.update({ stage: 'error', detail: error.message || String(error) });
        }
    }

    // 取消仅发出请求，收到下载器取消回执后才恢复可重试状态。
    async cancelOrClose() {
        if (['error', 'cancelled'].includes(this.stage)) {
            this.close();
            return;
        }
        el('updateBtnCancel').disabled = true;
        el('updateStageText').textContent = this.t('update.cancelling');
        try {
            await api('cancel_update_download');
        } catch (error) {
            this.update({ stage: 'error', detail: error.message || String(error) });
        }
    }

    // 已交接替换程序后只展示状态，由原事务完成退出与重启。
    update(event) {
        this.stage = event.stage;
        const detail = event.detail || '';
        if (event.stage === 'downloading') {
            this.progress(event);
            return;
        }
        const key = this.statusKey(event);
        el('updateStageText').textContent = this.t(key, { error: detail });
        const finished = ['error', 'cancelled'].includes(event.stage);
        if (['verifying', 'applying'].includes(event.stage)) el('updateProgressBar').style.width = '100%';
        el('updateBtnCancel').disabled = ['verifying', 'applying'].includes(event.stage);
        el('updateBtnCancel').textContent = this.t(finished ? 'dialog.ok' : 'update.cancel');
        if (finished) this.toast(this.t(key, { error: detail }));
    }

    // 下载器各阶段使用明确的文案映射，未知阶段必须显式报错。
    statusKey(event) {
        if (event.stage === 'connecting') {
            return event.detail === 'official' ? 'update.connecting_official' : 'update.connecting_mirror';
        }
        const keys = { retrying: 'update.retrying', fallback: 'update.fallback', verifying: 'update.verifying',
            applying: 'update.applying', cancelled: 'update.cancelled', error: 'update.failed' };
        if (!keys[event.stage]) throw new Error(`Unknown update stage: ${event.stage}`);
        return keys[event.stage];
    }

    // 仅接收实际回调推进进度与速率，不通过计时模拟下载。
    progress(event) {
        el('updateProgressBar').style.width = `${event.percent}%`;
        el('updateStageText').textContent = this.t('update.downloading', { percent: event.percent });
        el('updateSizeStat').textContent = `${(event.downloaded / BYTES_PER_MIB).toFixed(1)} / ${(event.total / BYTES_PER_MIB).toFixed(1)} MB (${event.percent}%)`;
        el('updateSpeedStat').textContent = `${(event.speed / BYTES_PER_MIB).toFixed(2)} MB/s`;
    }

    // 关闭说明或已完成的下载提示；取消后的重试重新取得发布信息。
    close() {
        if (!['notes', 'error', 'cancelled'].includes(this.stage)) return;
        el('updateModalOverlay').style.display = 'none';
        this.info = null;
        this.stage = 'idle';
    }
}
