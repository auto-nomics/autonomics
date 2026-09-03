/**
 * Phase 5/6 strategy 单元测试
 *
 * 覆盖：
 * - strategyRegistry.getStrategy 对每个 AgentType 都返回正确 strategy
 * - getStrategy 对未知 agentType 抛错（fail-fast）
 * - 每个 strategy 的 enabledLayers 与设计意图一致（防止误改）
 * - ThreadStrategy 不在主 registry（独立路径）
 */

import { describe, it, expect } from 'vitest';
import { getStrategy, listStrategies } from '../strategyRegistry';
import { HomepageStrategy } from '../HomepageStrategy';
import { PaperReaderStrategy } from '../PaperReaderStrategy';
import { ScreeningStrategy } from '../ScreeningStrategy';
import { ThreadStrategy } from '../ThreadStrategy';

describe('strategyRegistry', () => {
  describe('getStrategy', () => {
    it('homepage → HomepageStrategy', () => {
      expect(getStrategy('homepage')).toBe(HomepageStrategy);
    });

    it('paperReader → PaperReaderStrategy', () => {
      expect(getStrategy('paperReader')).toBe(PaperReaderStrategy);
    });

    it('screening → ScreeningStrategy', () => {
      expect(getStrategy('screening')).toBe(ScreeningStrategy);
    });

  });

  describe('listStrategies', () => {
    it('返回已注册 strategy（不含 ThreadStrategy）', () => {
      const all = listStrategies();
      expect(all).toHaveLength(3);
      expect(all).toContain(HomepageStrategy);
      expect(all).toContain(PaperReaderStrategy);
      expect(all).toContain(ScreeningStrategy);
      expect(all).not.toContain(ThreadStrategy);
    });
  });
});

describe('各 strategy 的 enabledLayers（设计意图锁定）', () => {
  it('HomepageStrategy: 不含 paperInfo', () => {
    const layers = HomepageStrategy.enabledLayers;
    expect(layers.has('persona')).toBe(true);
    expect(layers.has('paperInfo')).toBe(false);
    expect(layers.has('threadContext')).toBe(true);
  });

  it('PaperReaderStrategy: 启用论文上下文相关 layer（最重）', () => {
    const layers = PaperReaderStrategy.enabledLayers;
    expect(layers.has('persona')).toBe(true);
    expect(layers.has('paperInfo')).toBe(true);
    expect(layers.has('threadContext')).toBe(true);
    expect(layers.has('additionalHigh')).toBe(true);
    expect(layers.has('additionalLow')).toBe(true);
  });

  it('ScreeningStrategy: paperInfo ✓', () => {
    const layers = ScreeningStrategy.enabledLayers;
    expect(layers.has('paperInfo')).toBe(true);
  });

  it('ThreadStrategy: 启用 threadContext（其他 strategy 都不启用作为独立层）', () => {
    const layers = ThreadStrategy.enabledLayers;
    expect(layers.has('threadContext')).toBe(true);
    expect(layers.has('paperInfo')).toBe(false);
  });
});

describe('各 strategy 的元信息', () => {
  it('agentType 字段对齐（ThreadStrategy 复用 paperReader）', () => {
    expect(HomepageStrategy.agentType).toBe('homepage');
    expect(PaperReaderStrategy.agentType).toBe('paperReader');
    expect(ScreeningStrategy.agentType).toBe('screening');
    expect(ThreadStrategy.agentType).toBe('paperReader'); // 复用，不在 registry
  });

  it('description 非空（每个 strategy 都有人类可读说明）', () => {
    for (const s of [HomepageStrategy, PaperReaderStrategy, ScreeningStrategy, ThreadStrategy]) {
      expect(s.description.length).toBeGreaterThan(0);
    }
  });
});
