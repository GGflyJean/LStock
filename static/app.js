(function () {
  function tickClock() {
    const clock = document.getElementById('utc-clock');
    if (!clock) return;
    const now = new Date();
    clock.textContent = now.toISOString().slice(11, 19) + ' UTC';
  }
  tickClock();
  setInterval(tickClock, 1000);

  const autoRefresh = document.querySelector('[data-auto-refresh]');
  if (autoRefresh) {
    const interval = Number(autoRefresh.dataset.autoRefresh) || 15000;
    setInterval(function () {
      window.location.reload();
    }, interval);
  }

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
      const text = await response.text();
      let data = {};
      try {
        data = JSON.parse(text);
      } catch (error) {
        data = {};
      }
      if (!response.ok) {
        if (status) status.textContent = text || '刷新失败，请重试';
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

(function () {
  function bucketOf(date, granularity) {
    const parts = String(date).split(/[-/: ]/);
    const year = Number(parts[0]);
    const month = Number(parts[1] || 1);
    if (!Number.isFinite(year)) return null;
    if (granularity === 'year') {
      return { key: String(year), sort: year };
    }
    if (granularity === 'half') {
      const half = month <= 6 ? 1 : 2;
      return { key: year + ' H' + half, sort: year * 10 + half };
    }
    if (granularity === 'quarter') {
      const quarter = Math.ceil(month / 3);
      return { key: year + ' Q' + quarter, sort: year * 10 + quarter };
    }
    return {
      key: year + '-' + String(month).padStart(2, '0'),
      sort: year * 100 + month,
    };
  }

  function aggregate(points, granularity) {
    const buckets = new Map();
    points.forEach(function (point) {
      if (!Number.isFinite(point.value)) return;
      const bucket = bucketOf(point.date, granularity);
      if (!bucket) return;
      if (!buckets.has(bucket.key)) {
        buckets.set(bucket.key, { key: bucket.key, sort: bucket.sort, sum: 0, count: 0 });
      }
      const item = buckets.get(bucket.key);
      item.sum += point.value;
      item.count += 1;
    });
    return Array.from(buckets.values())
      .sort(function (a, b) { return a.sort - b.sort; })
      .map(function (item) {
        return { key: item.key, value: item.sum / item.count };
      });
  }

  function drawMacroChart(canvas, points, unit) {
    const dpr = window.devicePixelRatio || 1;
    const width = Math.max(canvas.clientWidth || 320, 240);
    const height = 180;
    canvas.width = Math.round(width * dpr);
    canvas.height = Math.round(height * dpr);
    const ctx = canvas.getContext('2d');
    ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
    ctx.clearRect(0, 0, width, height);

    if (!points.length) {
      ctx.fillStyle = '#8b9bb1';
      ctx.font = '13px -apple-system, BlinkMacSystemFont, sans-serif';
      ctx.fillText('暂无历史数据', 12, 28);
      return;
    }

    const padding = { left: 48, right: 12, top: 12, bottom: 26 };
    const plotWidth = width - padding.left - padding.right;
    const plotHeight = height - padding.top - padding.bottom;
    const values = points.map(function (point) { return point.value; });
    let min = Math.min.apply(null, values);
    let max = Math.max.apply(null, values);
    if (min === max) {
      min -= 1;
      max += 1;
    }
    const span = max - min;
    min -= span * 0.08;
    max += span * 0.08;
    const x = function (index) {
      return padding.left + (points.length === 1 ? plotWidth / 2 : plotWidth * index / (points.length - 1));
    };
    const y = function (value) {
      return padding.top + plotHeight * (1 - (value - min) / (max - min));
    };

    ctx.strokeStyle = '#2a3546';
    ctx.lineWidth = 1;
    ctx.fillStyle = '#8b9bb1';
    ctx.font = '11px -apple-system, BlinkMacSystemFont, sans-serif';
    for (let i = 0; i <= 4; i++) {
      const value = min + (max - min) * i / 4;
      const py = y(value);
      ctx.beginPath();
      ctx.moveTo(padding.left, py);
      ctx.lineTo(width - padding.right, py);
      ctx.stroke();
      ctx.fillText(value.toFixed(1) + (unit || ''), 4, py + 4);
    }

    if (min < 0 && max > 0) {
      ctx.save();
      ctx.strokeStyle = '#8b9bb1';
      ctx.setLineDash([4, 4]);
      ctx.beginPath();
      ctx.moveTo(padding.left, y(0));
      ctx.lineTo(width - padding.right, y(0));
      ctx.stroke();
      ctx.restore();
    }

    ctx.beginPath();
    points.forEach(function (point, index) {
      if (index === 0) {
        ctx.moveTo(x(index), y(point.value));
      } else {
        ctx.lineTo(x(index), y(point.value));
      }
    });
    ctx.strokeStyle = '#4c8dff';
    ctx.lineWidth = 2;
    ctx.stroke();

    ctx.lineTo(x(points.length - 1), padding.top + plotHeight);
    ctx.lineTo(x(0), padding.top + plotHeight);
    ctx.closePath();
    const gradient = ctx.createLinearGradient(0, padding.top, 0, padding.top + plotHeight);
    gradient.addColorStop(0, 'rgba(76, 141, 255, 0.25)');
    gradient.addColorStop(1, 'rgba(76, 141, 255, 0.02)');
    ctx.fillStyle = gradient;
    ctx.fill();

    const labels = Math.min(6, points.length);
    ctx.fillStyle = '#8b9bb1';
    ctx.font = '11px -apple-system, BlinkMacSystemFont, sans-serif';
    for (let i = 0; i < labels; i++) {
      const index = labels === 1 ? 0 : Math.round(i * (points.length - 1) / (labels - 1));
      const label = points[index].key;
      const labelX = x(index);
      ctx.fillText(label, Math.min(Math.max(labelX - 18, padding.left), width - 60), height - 8);
    }
  }

  function renderMacroPane(pane) {
    const select = pane.querySelector('.macro-granularity');
    const granularity = select ? select.value : 'year';
    pane.querySelectorAll('.macro-canvas').forEach(function (canvas) {
      let points = [];
      try {
        points = JSON.parse(canvas.dataset.series || '[]');
      } catch (error) {
        points = [];
      }
      drawMacroChart(canvas, aggregate(points, granularity), canvas.dataset.unit || '');
    });
  }

  function renderAllMacroPanes() {
    document.querySelectorAll('.macro-pane').forEach(renderMacroPane);
  }

  document.querySelectorAll('.macro-pane').forEach(function (pane) {
    const select = pane.querySelector('.macro-granularity');
    if (select) {
      select.addEventListener('change', function () { renderMacroPane(pane); });
    }
  });
  renderAllMacroPanes();

  let resizeTimer = null;
  window.addEventListener('resize', function () {
    clearTimeout(resizeTimer);
    resizeTimer = setTimeout(renderAllMacroPanes, 150);
  });
})();

(function () {
  const FACTOR_INFO = {
    momentum: { title: '动量', rule: '统计区间内价格涨跌幅，衡量短期趋势强度。' },
    relative_strength: { title: '相对强度', rule: '相对候选池平均表现的超额涨跌幅，越高说明越强。' },
    volume_ratio: { title: '量比', rule: '当前成交量 / 近期平均成交量。大于 1 表示放量。' },
    turnover: { title: '换手率', rule: '成交量 / 流通股本 × 100%，反映交易活跃度。' },
    fund_flow: { title: '资金流', rule: '主力资金净流入金额，正值表示净流入。' },
    breakout: { title: '突破距离', rule: '当前价相对近期高点的位置，越接近突破越高。' },
    volatility: { title: '波动率', rule: '近期价格波动幅度；越低越稳定，打分时按越低越好处理。' },
    revenue_growth: { title: '营收增速', rule: '营业收入同比增速，来自最新财报。' },
    eps_growth: { title: 'EPS 增速', rule: '每股收益同比增速；数据源缺失时使用净利润同比增速。' },
    roe: { title: 'ROE', rule: '净资产收益率，净利润 / 净资产 × 100%。' },
    roic: { title: 'ROIC', rule: '投入资本回报率，衡量投入资本的盈利效率。金融股通常不披露。' },
    gross_margin_trend: { title: '毛利率趋势', rule: '毛利率同比变化，正值表示毛利率提升。' },
    free_cash_flow: { title: '自由现金流', rule: '企业自由现金流（FCFF），越高越好。' },
    debt_to_equity: { title: '杠杆', rule: '由资产负债率换算的负债/权益比；越低越稳健。' },
    valuation_percentile: { title: '估值(PE)', rule: '市盈率 PE；越低估值越便宜。亏损股市盈率为负或缺失。' },
    dividend_yield: { title: '股息率', rule: '最近年度每股现金分红 / 当前股价 × 100%。股价缺失时无法计算。' },
    payout_ratio: { title: '派息率', rule: '每股现金分红 / 每股收益 × 100%。' },
    dividend_years: { title: '连续分红年数', rule: '连续实施现金分红的年份数。' },
    dividend_growth: { title: '股息增速', rule: '最近年度每股分红相对上一年度的增速。' },
    fcf_coverage: { title: 'FCF 覆盖', rule: '自由现金流 / 年度现金分红总额，衡量分红可持续性。' },
  };

  let popover = null;

  function hidePopover() {
    if (popover) popover.style.display = 'none';
  }

  function showPopover(anchor, key) {
    const info = FACTOR_INFO[key] || { title: key, rule: '暂无说明。' };
    if (!popover) {
      popover = document.createElement('div');
      popover.className = 'info-popover';
      document.body.appendChild(popover);
    }
    popover.textContent = '';
    const title = document.createElement('strong');
    title.textContent = info.title;
    const rule = document.createElement('div');
    rule.textContent = info.rule;
    popover.appendChild(title);
    popover.appendChild(rule);
    popover.style.display = 'block';

    const rect = anchor.getBoundingClientRect();
    const width = 320;
    const left = Math.min(window.innerWidth - width - 8, Math.max(8, rect.left - 80));
    const top = Math.min(window.innerHeight - 120, rect.bottom + 8);
    popover.style.left = left + 'px';
    popover.style.top = top + 'px';
  }

  document.addEventListener('click', function (event) {
    if (!(event.target instanceof Element)) return;
    const icon = event.target.closest('.info-icon');
    if (icon) {
      event.preventDefault();
      event.stopPropagation();
      showPopover(icon, icon.dataset.infoKey);
      return;
    }
    if (popover && !event.target.closest('.info-popover')) hidePopover();
  });

  document.addEventListener('keydown', function (event) {
    if (event.key === 'Escape') {
      hidePopover();
      return;
    }
    if ((event.key === 'Enter' || event.key === ' ') && event.target.classList.contains('info-icon')) {
      event.preventDefault();
      showPopover(event.target, event.target.dataset.infoKey);
    }
  });

  window.addEventListener('scroll', hidePopover, true);
  window.addEventListener('resize', hidePopover);
})();
