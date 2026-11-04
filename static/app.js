(function () {
  function tickClock() {
    const clock = document.getElementById('utc-clock');
    if (!clock) return;
    const now = new Date();
    clock.textContent = now.toISOString().slice(11, 19) + ' UTC';
  }
  tickClock();
  setInterval(tickClock, 1000);

  const button = document.getElementById('refresh-btn');
  if (!button) return;
  const status = document.getElementById('refresh-status');

  button.addEventListener('click', async function () {
    if (button.disabled) return;
    const original = button.textContent;
    button.disabled = true;
    button.textContent = '刷新中…';
    if (status) status.textContent = '正在刷新当前页面…';

    const view = button.dataset.market || 'dual';
    const market = view === 'dual' ? 'both' : view;
    const body = new URLSearchParams({
      csrf: button.dataset.csrf || '',
      market: market,
      tab: button.dataset.tab || 'watch',
      board: button.dataset.board || '',
    });

    try {
      const response = await fetch('/api/refresh', {
        method: 'POST',
        headers: { 'Content-Type': 'application/x-www-form-urlencoded' },
        body: body.toString(),
      });
      const data = await response.json().catch(function () { return {}; });
      if (!response.ok) {
        if (status) status.textContent = '刷新失败，请重试';
        button.disabled = false;
        button.textContent = original;
        return;
      }
      const outcomes = data.outcomes || [];
      const cooldown = outcomes.find(function (item) { return item.status === 'cooldown'; });
      if (cooldown) {
        if (status) status.textContent = '刚刚刷新过，请 ' + cooldown.remaining_secs + ' 秒后再试';
        button.disabled = false;
        button.textContent = original;
        return;
      }
      const failed = outcomes.find(function (item) { return item.status === 'failed'; });
      if (failed) {
        if (status) status.textContent = '刷新失败：' + (failed.error || '未知错误');
        button.disabled = false;
        button.textContent = original;
        return;
      }
      if (status) status.textContent = '刷新完成，正在更新页面…';
      window.location.reload();
    } catch (error) {
      if (status) status.textContent = '刷新失败：' + error;
      button.disabled = false;
      button.textContent = original;
    }
  });
})();
