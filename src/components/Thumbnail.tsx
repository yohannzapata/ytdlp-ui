import { useState } from "react";
import { makeStyles, mergeClasses, tokens } from "@fluentui/react-components";
import { MusicNote2Regular, VideoRegular } from "@fluentui/react-icons";

const useStyles = makeStyles({
  frame: {
    position: "relative",
    flexShrink: 0,
    aspectRatio: "16 / 9",
    borderRadius: tokens.borderRadiusMedium,
    overflow: "hidden",
    backgroundColor: tokens.colorNeutralBackground3,
    color: tokens.colorNeutralForeground3,
    display: "flex",
    alignItems: "center",
    justifyContent: "center",
  },
  image: {
    position: "absolute",
    inset: 0,
    width: "100%",
    height: "100%",
    objectFit: "cover",
  },
});

interface Props {
  src: string | null;
  width: number;
  audio?: boolean;
  className?: string;
}

export function Thumbnail({ src, width, audio, className }: Props) {
  const styles = useStyles();
  const [failed, setFailed] = useState(false);
  const Icon = audio ? MusicNote2Regular : VideoRegular;

  return (
    <div className={mergeClasses(styles.frame, className)} style={{ width }}>
      <Icon fontSize={width > 100 ? 28 : 18} />
      {src && !failed && (
        <img
          className={styles.image}
          src={src}
          alt=""
          loading="lazy"
          referrerPolicy="no-referrer"
          onError={() => setFailed(true)}
        />
      )}
    </div>
  );
}
