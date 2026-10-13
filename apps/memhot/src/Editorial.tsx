import { Icon } from "./Icon";
import { Markdown } from "./Markdown";
import { ReportDocument } from "./ReportDocument";
import { ReportContents } from "./ReportContents";
import {
  Accordion,
  AccordionItem,
  AccordionTrigger,
  AccordionContent,
} from "./ui/accordion";
import type { EditorialStory, Report } from "./types";

export function Editorial({
  report,
  onDiscuss,
  onMaterial,
  onEvent,
}: {
  report: Report;
  onDiscuss: (question: string) => void;
  onMaterial: (id: string) => void;
  onEvent: (id: string) => void;
}) {
  const editorial = report.editorial;
  if (!editorial) return <ReportDocument text={report.body} />;
  const stories = editorial.stories;
  const lead =
    stories.find((s) => s.id === editorial.leadStoryId) || stories[0];
  const sections = editorial.sections?.length
    ? editorial.sections
    : [
        ...new Set(
          stories
            .filter((s) => s !== lead)
            .map((s) => s.category || "值得关注"),
        ),
      ].map((title) => ({
        id: title,
        title,
        storyIds: stories
          .filter((s) => s !== lead && (s.category || "值得关注") === title)
          .map((s) => s.id || s.title),
      }));
  const rendered = new Set<string>();
  const anchor = (story: EditorialStory) => `event-${stories.indexOf(story)}`;
  const renderStory = (story: EditorialStory, featured = false) => {
    const key = story.id || story.title;
    if (rendered.has(key)) return null;
    rendered.add(key);
    const sources = report.sources.filter((source) =>
      story.evidenceIds?.includes(source.id),
    );
    return (
      <section
        className={`edition-story ${featured ? "edition-lead" : ""}`}
        id={anchor(story)}
        key={key}
      >
        <h2>{story.title}</h2>
        <div className="story-summary">
          <Markdown text={story.summary} />
        </div>
        {story.whyItMatters && (
          <div className="story-perspective">
            <h3>
              为什么值得关注 <span>编辑解读</span>
            </h3>
            <p>{story.whyItMatters}</p>
          </div>
        )}
        {story.readingGuide && (
          <p className="story-guide">
            <strong>阅读提示</strong> {story.readingGuide}
          </p>
        )}
        {sources.length > 0 && (
          <Accordion className="story-materials" type="single" collapsible>
            <AccordionItem value="sources">
              <AccordionTrigger>
                资料与出处 · {sources.length} 份
              </AccordionTrigger>
              <AccordionContent>
                {sources.map((source) => (
                  <button
                    key={source.id}
                    className="story-source"
                    onClick={() => onMaterial(source.id)}
                  >
                    <span>
                      <small>
                        {source.publisher || "原始来源"}
                        {source.retrievalKind === "metadata_or_abstract"
                          ? " · 摘要"
                          : ""}
                      </small>
                      <strong>{source.title}</strong>
                    </span>
                    <Icon name="arrow" size={17} />
                  </button>
                ))}
              </AccordionContent>
            </AccordionItem>
          </Accordion>
        )}
        <div className="story-actions">
          <button
            className="text-button"
            onClick={() =>
              onDiscuss(
                `关于「${story.title}」，请结合证据和限制，与我一起分析这件事。`,
              )
            }
          >
            <Icon name="chat" size={17} />
            聊聊这件事
          </button>
          {story.id && (
            <button className="text-button" onClick={() => onEvent(story.id!)}>
              <Icon name="clock" size={16} />
              相关进展
            </button>
          )}
          {(story.coverageCount || 0) > 1 && (
            <small>{story.coverageCount} 个来源</small>
          )}
        </div>
      </section>
    );
  };
  return (
    <div className="editorial">
      <ReportContents
        label="本期目录"
        entries={[
          ...stories.map((story) => ({
            id: anchor(story),
            title: story.title,
          })),
          ...(editorial.flashes?.length
            ? [
                {
                  id: "edition-flashes",
                  title: `快讯 · ${editorial.flashes.length} 条`,
                },
              ]
            : []),
        ]}
      />
      {lead && renderStory(lead, true)}
      {sections.map((section) => {
        const selected = stories.filter(
          (s) =>
            s !== lead &&
            section.storyIds.includes(s.id || s.title) &&
            !rendered.has(s.id || s.title),
        );
        return (
          selected.length > 0 && (
            <div className="edition-section" key={section.id}>
              <h2 className="edition-section-title">
                {section.title}
                <span>{selected.length}</span>
              </h2>
              {selected.map((s) => renderStory(s))}
            </div>
          )
        );
      })}
      {stories
        .filter((s) => !rendered.has(s.id || s.title))
        .map((s) => renderStory(s))}
      {!!editorial.flashes?.length && (
        <section className="edition-flashes" id="edition-flashes">
          <h2 className="edition-section-title">
            快讯<span>{editorial.flashes.length}</span>
          </h2>
          {editorial.flashes.map((story, i) => (
            <div className="edition-flash" key={story.id || i}>
              <h3>{story.title}</h3>
              {story.summary && <Markdown text={story.summary} />}
              <div className="story-actions">
                {story.evidenceIds?.[0] && (
                  <button
                    className="text-button"
                    onClick={() => onMaterial(story.evidenceIds![0])}
                  >
                    查看资料 <Icon name="arrow" size={15} />
                  </button>
                )}
                {story.id && (
                  <button
                    className="text-button"
                    onClick={() => onEvent(story.id!)}
                  >
                    相关进展
                  </button>
                )}
              </div>
            </div>
          ))}
        </section>
      )}
      {editorial.coverageNote && (
        <p className="edition-coverage">{editorial.coverageNote}</p>
      )}
    </div>
  );
}
