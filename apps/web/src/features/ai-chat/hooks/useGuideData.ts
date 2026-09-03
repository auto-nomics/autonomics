/**
 * 导览数据加载 Hook
 *
 * 负责加载和管理论文导览相关的数据，包括：
 * - 段落导览数据（V2 树 → V1 格式转换）
 * - 章节语义元数据
 * - 图片/表格上下文数据
 *
 * 从 ChatPanel 组件中提取，使代码结构更清晰，职责更单一。
 *
 * @module ai-chat/hooks/useGuideData
 */

import { useState, useCallback, useEffect } from 'react';
import type { Paper } from '@/types';
import { getGuideTree } from '../../../services/guideApi';
import { STANDALONE_CHAT_ID } from '../utils/chatConstants';
import type { Block } from '@/types';
import { getBlocks } from '../../../services/guideApi';

/** V2 导览节点 */
interface GuideNode {
  id: string;
  title?: string;
  summary?: string;
  role?: string;
  tags?: string[];
  importance?: number;
  original_text?: string;
  children?: GuideNode[];
}

/** V1 段落导览格式 */
interface V1Section {
  section_title: string;
  paragraphs: Array<{
    summary: string;
    importance: number;
    original_text: string;
  }>;
}

/** 章节语义元数据 */
interface SectionSemantic {
  title: string;
  index: number;
  topics: string[];
  contentType: string;
  mainFinding: string;
}

/** 图片/表格块 */
interface ImageBlock {
  blockType: string;
  pageIdx: number;
  imgPath?: string;
  text?: string;
  tableCaption?: string;
}

/** useGuideData 参数 */
interface UseGuideDataParams {
  paperId: number;
  paper: Paper | null;
}

/**
 * 导览数据加载 Hook
 *
 * @param {object} params - Hook 参数
 * @param {number} params.paperId - 论文 ID
 * @param {object} params.paper - 论文完整信息（用于检查解析状态）
 * @returns {object} 包含导览数据和操作函数的对象
 */
export function useGuideData({ paperId, paper }: UseGuideDataParams) {
  // ========== 状态定义 ==========

  // 段落导览数据状态
  const [guideSections, setGuideSections] = useState<V1Section[]>([]);

  // 章节语义元数据状态
  const [sectionSemantics, setSectionSemantics] = useState<SectionSemantic[] | null>(null);

  // 图片/表格上下文状态
  const [imageSystemContext, setImageSystemContext] = useState<Array<{ text: string; priority: string }>>([]);

  // ========== 数据转换函数 ==========

  /**
   * 从 V2 导览树构建章节语义元数据
   *
   * @param {Array} tree - V2 导览树数组
   * @returns {Array|null} 章节语义数组，无数据时返回 null
   */
  const buildSectionSemantics = useCallback((tree: GuideNode[]): SectionSemantic[] | null => {
    if (!tree || tree.length === 0) return null;

    const semantics: SectionSemantic[] = [];
    let index = 0;

    const traverse = (nodes: GuideNode[]) => {
      for (const node of nodes) {
        if (node.summary && node.summary.trim()) {
          semantics.push({
            title: node.title || '',
            index: index++,
            topics: Array.isArray(node.tags) ? node.tags : [],
            contentType: node.role || 'section',
            mainFinding: node.summary.substring(0, 200),
          });
        }
        if (node.children && node.children.length > 0) {
          traverse(node.children);
        }
      }
    };

    traverse(tree);
    return semantics.length > 0 ? semantics : null;
  }, []);

  /**
   * 将 V2 导览树转换为 V1 段落导览格式
   *
   * @param {Array} tree - V2 导览树数组
   * @returns {Array} V1 格式的章节段落数组
   */
  const transformV2TreeToV1Sections = useCallback((tree: GuideNode[]): V1Section[] => {
    if (!tree || tree.length === 0) return [];

    const sections: V1Section[] = [];

    const traverse = (nodes: GuideNode[], currentSectionTitle: string = '未分类') => {
      for (const node of nodes) {
        if (node.summary && node.summary.trim()) {
          const sectionTitle = node.title || currentSectionTitle;

          let section = sections.find(s => s.section_title === sectionTitle);
          if (!section) {
            section = { section_title: sectionTitle, paragraphs: [] };
            sections.push(section);
          }

          section.paragraphs.push({
            summary: node.summary,
            importance: node.importance || 3,
            original_text: node.original_text || '',
          });
        }

        if (node.children && node.children.length > 0) {
          const newSectionTitle = node.title || currentSectionTitle;
          traverse(node.children, newSectionTitle);
        }
      }
    };

    traverse(tree);
    return sections;
  }, []);

  // ========== 数据加载 ==========

  useEffect(() => {
    // paperId 无效时跳过加载，避免无意义的 404 请求
    if (
      !paperId ||
      paperId === -1 ||
      typeof paperId !== 'number' ||
      String(paperId) === STANDALONE_CHAT_ID
    ) {
      setGuideSections([]);
      setSectionSemantics(null);
      setImageSystemContext([]);
      return;
    }

    // 创建 AbortController 用于取消请求
    const controller = new AbortController();

    const loadGuideData = async () => {
      try {
        const guideTreeData = await getGuideTree(String(paperId), controller.signal).catch(() => ({ nodes: [] }));

        // 检查组件是否已卸载或请求是否已取消
        if (controller.signal.aborted) return;

        // 转换为 V1 格式
        const v1Sections = transformV2TreeToV1Sections((guideTreeData.nodes || []) as any as GuideNode[]);
        setGuideSections(v1Sections);

        // 构建语义元数据
        const semantics = buildSectionSemantics((guideTreeData.nodes || []) as any as GuideNode[]);
        setSectionSemantics(semantics);

        // 加载图片/表格上下文
        if (paper?.parse_status === 'done') {
          try {
            const blocksData = await getBlocks(String(paperId), null, controller.signal);
            // 再次检查是否已取消
            if (controller.signal.aborted) return;

            const imageBlocks = ((blocksData?.blocks || []) as any[]).filter(
              (b: any) => (b.blockType === 'image' || b.blockType === 'table') && b.imgPath
            );
            if (imageBlocks.length > 0) {
              const figureList = imageBlocks
                .filter((b: any) => b.blockType === 'image')
                .map((b: any) => `- 图片 (第${b.pageIdx + 1}页): ${b.tableCaption || b.text || '无标题'}`)
                .join('\n');
              const tableList = imageBlocks
                .filter((b: any) => b.blockType === 'table')
                .map((b: any) => `- 表格 (第${b.pageIdx + 1}页): ${b.tableCaption || b.text || '无标题'}`)
                .join('\n');
              // 最后检查是否已取消
              if (controller.signal.aborted) return;

              setImageSystemContext([{
                text: `论文包含以下图表（用户可能会问相关内容）：\n${figureList}\n${tableList}`,
                priority: 'high',
              }]);
            }
          } catch {
            // 获取失败不影响主流程
          }
        }
      } catch (err: any) {
        // 忽略 AbortError，只记录其他错误
        if (err.name !== 'AbortError') {
          console.error('加载导览数据失败:', err);
        }
      }
    };

    loadGuideData();

    // 清理函数：组件卸载时取消进行中的请求
    return () => {
      controller.abort();
    };
  }, [paperId, paper, transformV2TreeToV1Sections, buildSectionSemantics]);

  return {
    guideSections,
    setGuideSections,
    sectionSemantics,
    setSectionSemantics,
    imageSystemContext,
    setImageSystemContext,
    buildSectionSemantics,
    transformV2TreeToV1Sections,
  };
}
